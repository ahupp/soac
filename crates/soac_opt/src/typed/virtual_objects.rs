use super::*;

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedFieldScalarizationStats {
    pub seeded_objects: usize,
    pub scalar_slots: usize,
    pub inserted_scalar_stores: usize,
    pub inserted_block_params: usize,
    pub inserted_block_args: usize,
    pub rewritten_loads: usize,
}

#[derive(Debug, Clone)]
pub struct TypedVirtualObjectPlan {
    pub object_id: TypedVirtualObjectId,
    pub source: InstrId,
    pub root: ResolvedName,
    pub field_bindings: TypedConstructorFieldBindings,
    pub materialization_recipe: Option<TypedVirtualMaterializationRecipe>,
    pub materialization_block: BlockLabel,
    pub materialization_index: usize,
    pub reachable_blocks: HashSet<BlockLabel>,
    pub virtual_locations: HashSet<LocalLocation>,
    pub virtual_names: HashSet<String>,
    pub assumed_owner_type: Option<TypedAttrOwnerRef>,
    pub guard_blocks: HashSet<BlockLabel>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct TypedVirtualObjectId(pub u32);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum TypedVirtualBoundaryLocation {
    BodyInstr {
        block: BlockLabel,
        instr_index: usize,
    },
    Term {
        block: BlockLabel,
    },
    ExceptionEdge {
        block: BlockLabel,
        target: BlockLabel,
    },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum TypedVirtualBoundaryKind {
    EscapingStore,
    VirtualNameRebind,
    UnsupportedBodyUse,
    DeoptResumeUse,
    UnsupportedGuard,
    BranchTestUse,
    RaiseUse,
    ReturnUse,
    EscapingEdge,
    UnsupportedEdgeParam,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct TypedVirtualMaterializationBoundary {
    pub source: InstrId,
    pub location: TypedVirtualBoundaryLocation,
    pub kind: TypedVirtualBoundaryKind,
}

#[derive(Debug, Clone)]
pub struct TypedVirtualMaterializationRecipe {
    pub allocation: InstrTyped,
    pub field_stores: Vec<TypedSetAttr<InstrTyped>>,
}

#[derive(Debug, Clone, Default)]
pub struct TypedVirtualizationPlan {
    pub objects: Vec<TypedVirtualObjectPlan>,
    pub materializing_objects: Vec<TypedVirtualObjectPlan>,
    field_lowering_bindings: HashMap<InstrId, TypedConstructorFieldBindings>,
    materialization_boundaries: Vec<TypedVirtualMaterializationBoundary>,
    pub field_states: Option<TypedVirtualFieldStateAnalysis>,
}

impl TypedVirtualizationPlan {
    fn has_field_lowering_candidates(&self) -> bool {
        !self.field_lowering_bindings.is_empty()
    }

    pub fn materialization_boundaries(&self) -> &[TypedVirtualMaterializationBoundary] {
        &self.materialization_boundaries
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct TypedVirtualFieldRef {
    pub object: TypedVirtualObjectId,
    pub field_name: String,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedVirtualState {
    pub aliases: HashMap<LocalLocation, TypedVirtualObjectId>,
    pub fields: HashMap<TypedVirtualFieldRef, ResolvedName>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct TypedVirtualFieldEdge {
    pub from: BlockLabel,
    pub to: BlockLabel,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct TypedVirtualBodyInstr {
    pub block: BlockLabel,
    pub instr_index: usize,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedVirtualFieldStateAnalysis {
    pub block_in: HashMap<BlockLabel, TypedVirtualState>,
    pub body_before_instr: HashMap<TypedVirtualBodyInstr, TypedVirtualState>,
    pub block_before_term: HashMap<BlockLabel, TypedVirtualState>,
    pub block_out: HashMap<BlockLabel, TypedVirtualState>,
    pub edge_out: HashMap<TypedVirtualFieldEdge, TypedVirtualState>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedVirtualConstructorStats {
    pub planned_objects: usize,
    pub removed_materializations: usize,
    pub removed_field_stores: usize,
    pub removed_alias_stores: usize,
    pub removed_dels: usize,
    pub removed_guards: usize,
    pub removed_block_params: usize,
    pub removed_block_args: usize,
}

impl TypedVirtualConstructorStats {
    pub fn changed(&self) -> bool {
        self.removed_materializations != 0
            || self.removed_field_stores != 0
            || self.removed_alias_stores != 0
            || self.removed_dels != 0
            || self.removed_guards != 0
            || self.removed_block_params != 0
            || self.removed_block_args != 0
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedVirtualReturnMaterializationStats {
    pub materialized_objects: usize,
    pub inserted_allocations: usize,
    pub inserted_field_stores: usize,
    pub virtualization: TypedVirtualConstructorStats,
}

impl TypedVirtualReturnMaterializationStats {
    pub fn changed(&self) -> bool {
        self.materialized_objects != 0 || self.virtualization.changed()
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedVirtualStoreMaterializationStats {
    pub materialized_objects: usize,
    pub inserted_allocations: usize,
    pub inserted_field_stores: usize,
    pub virtualization: TypedVirtualConstructorStats,
}

impl TypedVirtualStoreMaterializationStats {
    pub fn changed(&self) -> bool {
        self.materialized_objects != 0 || self.virtualization.changed()
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedVirtualBodyMaterializationStats {
    pub materialized_objects: usize,
    pub inserted_allocations: usize,
    pub inserted_field_stores: usize,
    pub virtualization: TypedVirtualConstructorStats,
}

impl TypedVirtualBodyMaterializationStats {
    pub fn changed(&self) -> bool {
        self.materialized_objects != 0 || self.virtualization.changed()
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedVirtualObjectLoweringStats {
    pub field_lowering: TypedFieldScalarizationStats,
    pub return_materialization: TypedVirtualReturnMaterializationStats,
    pub store_materialization: TypedVirtualStoreMaterializationStats,
    pub body_materialization: TypedVirtualBodyMaterializationStats,
    pub virtualization: TypedVirtualConstructorStats,
}

impl TypedVirtualObjectLoweringStats {
    pub fn changed(&self) -> bool {
        self.field_lowering.seeded_objects != 0
            || self.field_lowering.scalar_slots != 0
            || self.field_lowering.inserted_scalar_stores != 0
            || self.field_lowering.inserted_block_params != 0
            || self.field_lowering.inserted_block_args != 0
            || self.field_lowering.rewritten_loads != 0
            || self.return_materialization.changed()
            || self.store_materialization.changed()
            || self.body_materialization.changed()
            || self.virtualization.changed()
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TypedFullyVirtualObjectLoweringStats {
    pub field_lowering: TypedFieldScalarizationStats,
    pub virtualization: TypedVirtualConstructorStats,
}

impl TypedFullyVirtualObjectLoweringStats {
    pub fn changed(&self) -> bool {
        self.field_lowering.seeded_objects != 0
            || self.field_lowering.scalar_slots != 0
            || self.field_lowering.inserted_scalar_stores != 0
            || self.field_lowering.inserted_block_params != 0
            || self.field_lowering.inserted_block_args != 0
            || self.field_lowering.rewritten_loads != 0
            || self.virtualization.changed()
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct TypedVirtualLoweringAnalysis {
    block_in: HashMap<BlockLabel, TypedVirtualLoweringState>,
    body_before_instr: HashMap<TypedVirtualBodyInstr, TypedVirtualLoweringState>,
    block_before_term: HashMap<BlockLabel, TypedVirtualLoweringState>,
    block_out: HashMap<BlockLabel, TypedVirtualLoweringState>,
    edge_out: HashMap<TypedVirtualFieldEdge, TypedVirtualLoweringState>,
}

impl TypedVirtualLoweringAnalysis {
    fn in_state(&self, label: BlockLabel) -> Option<&TypedVirtualLoweringState> {
        self.block_in.get(&label)
    }
}

fn analyze_typed_virtual_field_states(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
) -> TypedVirtualLoweringAnalysis {
    let predecessors = typed_scalar_block_predecessor_edges(function);
    let mut in_states = vec![None::<TypedVirtualLoweringState>; function.blocks.len()];
    let mut body_before_instr_states =
        vec![None::<Vec<TypedVirtualLoweringState>>; function.blocks.len()];
    let mut before_term_states = vec![None::<TypedVirtualLoweringState>; function.blocks.len()];
    let mut out_states = vec![None::<TypedVirtualLoweringState>; function.blocks.len()];
    let labels = typed_block_indices_by_label(function);
    let entry_label = function.blocks.first().map(|block| block.label);
    loop {
        let mut changed = false;
        for (block_index, block) in function.blocks.iter().enumerate() {
            let in_state = typed_field_scalar_in_state_for_block(
                function,
                block,
                entry_label,
                &predecessors,
                &labels,
                &out_states,
            );
            if in_states[block_index] != in_state {
                in_states[block_index] = in_state.clone();
                changed = true;
            }
            let (body_before_instr_state, before_term_state, out_state) = in_state
                .map(|mut out_state| {
                    let mut block_clone = block.clone();
                    let mut ignored_stats = TypedFieldScalarizationStats::default();
                    let mut snapshot_state = out_state.clone();
                    let body_before_instr_state = typed_field_scalar_body_snapshots(
                        block,
                        &mut snapshot_state,
                        module_constants,
                        constructor_field_bindings,
                    );
                    transfer_typed_field_scalar_body(
                        &mut block_clone.body,
                        &mut out_state,
                        module_constants,
                        constructor_field_bindings,
                        &mut ignored_stats,
                        false,
                    );
                    let before_term_state = out_state.clone();
                    transfer_typed_field_scalar_term(
                        &mut block_clone.term,
                        &mut out_state,
                        module_constants,
                        &mut ignored_stats,
                        false,
                    );
                    (body_before_instr_state, before_term_state, out_state)
                })
                .map_or(
                    (None, None, None),
                    |(body_before_instr_state, before_term_state, out_state)| {
                        (
                            Some(body_before_instr_state),
                            Some(before_term_state),
                            Some(out_state),
                        )
                    },
                );
            if body_before_instr_states[block_index] != body_before_instr_state {
                body_before_instr_states[block_index] = body_before_instr_state;
                changed = true;
            }
            if before_term_states[block_index] != before_term_state {
                before_term_states[block_index] = before_term_state;
                changed = true;
            }
            if out_states[block_index] != out_state {
                out_states[block_index] = out_state;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let block_in = function
        .blocks
        .iter()
        .zip(in_states)
        .filter_map(|(block, state)| state.map(|state| (block.label, state)))
        .collect::<HashMap<_, _>>();
    let block_out = function
        .blocks
        .iter()
        .zip(out_states)
        .filter_map(|(block, state)| state.map(|state| (block.label, state)))
        .collect::<HashMap<_, _>>();
    let block_before_term = function
        .blocks
        .iter()
        .zip(before_term_states)
        .filter_map(|(block, state)| state.map(|state| (block.label, state)))
        .collect::<HashMap<_, _>>();
    let body_before_instr =
        function
            .blocks
            .iter()
            .zip(body_before_instr_states)
            .flat_map(|(block, states)| {
                states.unwrap_or_default().into_iter().enumerate().map(
                    move |(instr_index, state)| {
                        (
                            TypedVirtualBodyInstr {
                                block: block.label,
                                instr_index,
                            },
                            state,
                        )
                    },
                )
            })
            .collect::<HashMap<_, _>>();
    let edge_out = function
        .blocks
        .iter()
        .flat_map(|block| {
            typed_scalar_term_successors(&block.term)
                .into_iter()
                .map(move |to| TypedVirtualFieldEdge {
                    from: block.label,
                    to,
                })
        })
        .filter_map(|edge| {
            block_out
                .get(&edge.from)
                .cloned()
                .map(|state| (edge, state))
        })
        .collect();
    TypedVirtualLoweringAnalysis {
        block_in,
        body_before_instr,
        block_before_term,
        block_out,
        edge_out,
    }
}

fn lower_typed_virtual_fields_to_locals_from_analysis(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
    scalar_slots: usize,
    analysis: &TypedVirtualLoweringAnalysis,
) -> TypedFieldScalarizationStats {
    let mut stats = TypedFieldScalarizationStats {
        scalar_slots,
        ..TypedFieldScalarizationStats::default()
    };
    for block in &mut function.blocks {
        let mut state = analysis
            .in_state(block.label)
            .cloned()
            .unwrap_or_else(TypedVirtualLoweringState::default);
        transfer_typed_field_scalar_block(
            block,
            &mut state,
            module_constants,
            constructor_field_bindings,
            &mut stats,
            true,
        );
    }
    stats
}

fn project_typed_virtual_field_states(
    analysis: &TypedVirtualLoweringAnalysis,
) -> TypedVirtualFieldStateAnalysis {
    TypedVirtualFieldStateAnalysis {
        block_in: analysis
            .block_in
            .iter()
            .map(|(label, state)| (*label, state.virtual_state.clone()))
            .collect(),
        body_before_instr: analysis
            .body_before_instr
            .iter()
            .map(|(location, state)| (*location, state.virtual_state.clone()))
            .collect(),
        block_before_term: analysis
            .block_before_term
            .iter()
            .map(|(label, state)| (*label, state.virtual_state.clone()))
            .collect(),
        block_out: analysis
            .block_out
            .iter()
            .map(|(label, state)| (*label, state.virtual_state.clone()))
            .collect(),
        edge_out: analysis
            .edge_out
            .iter()
            .map(|(edge, state)| (*edge, state.virtual_state.clone()))
            .collect(),
    }
}

pub fn plan_typed_virtual_objects(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
) -> TypedVirtualizationPlan {
    plan_typed_virtual_objects_impl(
        function,
        module_constants,
        constructor_field_bindings,
        None,
        true,
    )
}

pub fn plan_typed_fully_virtual_objects(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
    trusted_sources: &HashSet<InstrId>,
) -> TypedVirtualizationPlan {
    plan_typed_virtual_objects_impl(
        function,
        module_constants,
        constructor_field_bindings,
        Some(trusted_sources),
        false,
    )
}

fn plan_typed_virtual_objects_impl(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
    allowed_sources: Option<&HashSet<InstrId>>,
    include_materializing_objects: bool,
) -> TypedVirtualizationPlan {
    if constructor_field_bindings.is_empty() {
        return TypedVirtualizationPlan::default();
    }
    let labels = typed_block_indices_by_label(function);
    let mut objects = Vec::new();
    let mut materializing_objects = Vec::new();
    let mut materialization_boundaries = Vec::new();
    let field_lowering_bindings = constructor_field_bindings
        .iter()
        .filter(|(source, _)| allowed_sources.is_none_or(|allowed| allowed.contains(source)))
        .map(|(source, bindings)| (*source, bindings.clone()))
        .collect::<HashMap<_, _>>();
    for block in &function.blocks {
        for (instr_index, instr) in block.body.iter().enumerate() {
            let Some((source, root)) = typed_virtual_constructor_materialization(instr) else {
                continue;
            };
            if allowed_sources.is_some_and(|allowed| !allowed.contains(&source)) {
                continue;
            }
            let Some(bindings) = constructor_field_bindings.get(&source) else {
                continue;
            };
            let Some(root_location) = root.local_location() else {
                continue;
            };
            let Some(reachable) = typed_virtual_reachable_blocks_after_materialization(
                function,
                &labels,
                &block.term,
                include_materializing_objects,
            ) else {
                continue;
            };
            if reachable.contains(&block.label)
                || reachable.len() > MAX_TYPED_HOT_CONTINUATION_CLONE_BLOCKS
            {
                continue;
            }
            let mut plan = TypedVirtualObjectPlan {
                object_id: TypedVirtualObjectId(source.index()),
                source,
                root: root.clone(),
                field_bindings: bindings.clone(),
                materialization_recipe: None,
                materialization_block: block.label,
                materialization_index: instr_index,
                reachable_blocks: reachable,
                virtual_locations: HashSet::from([root_location]),
                virtual_names: HashSet::from([root.id_str().to_string()]),
                assumed_owner_type: None,
                guard_blocks: HashSet::new(),
            };
            plan.materialization_recipe =
                typed_virtual_materialization_recipe(function, module_constants, bindings, &plan);
            if let Err(boundary) = complete_typed_virtual_constructor_plan(
                function,
                module_constants,
                bindings,
                &mut plan,
            ) {
                if include_materializing_objects
                    && plan.materialization_recipe.as_ref().is_some_and(|recipe| {
                        typed_virtual_materialization_recipe_inputs_bound_at_boundary(
                            function, recipe, boundary,
                        )
                    })
                {
                    materialization_boundaries.push(boundary);
                    materializing_objects.push(plan);
                }
                continue;
            }
            objects.push(plan);
        }
    }
    let field_states = project_typed_virtual_field_states(&analyze_typed_virtual_field_states(
        function,
        module_constants,
        &field_lowering_bindings,
    ));
    let plan = TypedVirtualizationPlan {
        objects,
        materializing_objects,
        field_lowering_bindings,
        materialization_boundaries,
        field_states: Some(field_states),
    };
    plan
}

fn typed_virtual_reachable_blocks_after_materialization(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    labels: &HashMap<BlockLabel, usize>,
    term: &BlockTerm<InstrTyped>,
    require_hot_continuation: bool,
) -> Option<HashSet<BlockLabel>> {
    match term {
        BlockTerm::Jump(edge) => typed_hot_reachable_block_labels(function, labels, edge.target),
        BlockTerm::Return(_) | BlockTerm::Raise(_) if !require_hot_continuation => {
            Some(HashSet::new())
        }
        BlockTerm::IfTerm(_)
        | BlockTerm::BranchTable(_)
        | BlockTerm::Return(_)
        | BlockTerm::Raise(_) => None,
    }
}

pub fn lower_typed_fully_virtual_objects_to_locals_with_plan(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    plan: &mut TypedVirtualizationPlan,
) -> TypedFullyVirtualObjectLoweringStats {
    let field_lowering =
        lower_typed_virtual_fields_to_locals_with_plan(function, module_constants, plan);
    let virtualization =
        virtualize_typed_hot_constructor_plans(function, module_constants, &plan.objects);
    TypedFullyVirtualObjectLoweringStats {
        field_lowering,
        virtualization,
    }
}

pub fn lower_typed_virtual_fields_to_locals_with_plan(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    plan: &mut TypedVirtualizationPlan,
) -> TypedFieldScalarizationStats {
    if !plan.has_field_lowering_candidates() {
        return TypedFieldScalarizationStats::default();
    }
    let mut constructor_field_bindings = plan.field_lowering_bindings.clone();
    let scalar_slots =
        allocate_typed_constructor_field_scalar_slots(function, &mut constructor_field_bindings);
    let mut analysis =
        analyze_typed_virtual_field_states(function, module_constants, &constructor_field_bindings);
    let removable_objects = plan
        .objects
        .iter()
        .map(|object| object.object_id)
        .collect::<HashSet<_>>();
    let mut block_param_stats = TypedVirtualFieldBlockParamStats::default();
    loop {
        let split_edges =
            split_typed_virtual_field_block_param_edges(function, &analysis, &removable_objects);
        if split_edges != 0 {
            analysis = analyze_typed_virtual_field_states(
                function,
                module_constants,
                &constructor_field_bindings,
            );
        }
        let next_block_param_stats =
            synthesize_typed_virtual_field_block_params(function, &analysis, &removable_objects);
        block_param_stats.inserted_block_params += next_block_param_stats.inserted_block_params;
        block_param_stats.inserted_block_args += next_block_param_stats.inserted_block_args;
        if next_block_param_stats.inserted_block_params != 0 {
            analysis = analyze_typed_virtual_field_states(
                function,
                module_constants,
                &constructor_field_bindings,
            );
            refresh_typed_virtual_field_block_param_args(function, &analysis);
            analysis = analyze_typed_virtual_field_states(
                function,
                module_constants,
                &constructor_field_bindings,
            );
        }
        if split_edges == 0 && next_block_param_stats.inserted_block_params == 0 {
            break;
        }
    }
    let mut stats = lower_typed_virtual_fields_to_locals_from_analysis(
        function,
        module_constants,
        &constructor_field_bindings,
        scalar_slots,
        &analysis,
    );
    stats.inserted_block_params = block_param_stats.inserted_block_params;
    stats.inserted_block_args = block_param_stats.inserted_block_args;
    plan.field_states = Some(project_typed_virtual_field_states(&analysis));
    plan.field_lowering_bindings = constructor_field_bindings;
    for object in plan
        .objects
        .iter_mut()
        .chain(plan.materializing_objects.iter_mut())
    {
        object.field_bindings = plan
            .field_lowering_bindings
            .get(&object.source)
            .cloned()
            .expect("virtual object plan should retain source bindings during lowering");
    }
    stats
}

pub fn lower_typed_virtual_objects_to_locals_with_plan(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    plan: &mut TypedVirtualizationPlan,
) -> TypedVirtualObjectLoweringStats {
    let field_lowering =
        lower_typed_virtual_fields_to_locals_with_plan(function, module_constants, plan);
    let return_materialization =
        materialize_typed_virtual_return_boundaries_with_plan(function, module_constants, plan);
    let store_materialization =
        materialize_typed_virtual_store_boundaries_with_plan(function, module_constants, plan);
    let body_materialization =
        materialize_typed_virtual_body_boundaries_with_plan(function, module_constants, plan);
    let virtualization =
        virtualize_typed_hot_constructor_plans(function, module_constants, &plan.objects);
    TypedVirtualObjectLoweringStats {
        field_lowering,
        return_materialization,
        store_materialization,
        body_materialization,
        virtualization,
    }
}

pub fn materialize_typed_virtual_return_boundaries_with_plan(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    plan: &TypedVirtualizationPlan,
) -> TypedVirtualReturnMaterializationStats {
    let Some(field_states) = plan.field_states.as_ref() else {
        return TypedVirtualReturnMaterializationStats::default();
    };
    let labels = typed_block_indices_by_label(function);
    let boundaries = plan
        .materialization_boundaries
        .iter()
        .filter_map(|boundary| match boundary.location {
            TypedVirtualBoundaryLocation::Term { block }
                if boundary.kind == TypedVirtualBoundaryKind::ReturnUse =>
            {
                Some((boundary.source, block))
            }
            TypedVirtualBoundaryLocation::BodyInstr { .. }
            | TypedVirtualBoundaryLocation::Term { .. }
            | TypedVirtualBoundaryLocation::ExceptionEdge { .. } => None,
        })
        .collect::<HashMap<_, _>>();
    let mut insertions = Vec::<(
        TypedVirtualObjectPlan,
        BlockLabel,
        TypedVirtualMaterializationRecipe,
        Vec<ResolvedName>,
    )>::new();
    for object in &plan.materializing_objects {
        let Some(block_label) = boundaries.get(&object.source).copied() else {
            continue;
        };
        let Some(recipe) = object.materialization_recipe.clone() else {
            continue;
        };
        let Some(block_index) = labels.get(&block_label).copied() else {
            continue;
        };
        let BlockTerm::Return(value) = &function.blocks[block_index].term else {
            continue;
        };
        if !typed_expr_loads_resolved_name(value, &object.root) {
            continue;
        }
        let Some(state) = field_states.block_before_term.get(&block_label) else {
            continue;
        };
        let field_values = object
            .field_bindings
            .fields
            .iter()
            .map(|field| {
                state
                    .fields
                    .get(&TypedVirtualFieldRef {
                        object: object.object_id,
                        field_name: field.field_name.clone(),
                    })
                    .cloned()
            })
            .collect::<Option<Vec<_>>>();
        let Some(field_values) = field_values else {
            continue;
        };
        insertions.push((object.clone(), block_label, recipe, field_values));
    }

    let eligible_plans = insertions
        .iter()
        .map(|(object, _, _, _)| object.clone())
        .collect::<Vec<_>>();
    let mut stats = TypedVirtualReturnMaterializationStats {
        virtualization: virtualize_typed_hot_constructor_plans(
            function,
            module_constants,
            &eligible_plans,
        ),
        ..TypedVirtualReturnMaterializationStats::default()
    };
    for (object, block_label, recipe, field_values) in insertions {
        let Some(block_index) = labels.get(&block_label).copied() else {
            continue;
        };
        let block = &mut function.blocks[block_index];
        block.body.push(recipe.allocation);
        stats.inserted_allocations += 1;
        for (mut field_store, field_value) in recipe.field_stores.into_iter().zip(field_values) {
            field_store.value = Box::new(typed_load_temp(&object.root));
            field_store.replacement = Box::new(typed_load_temp(&field_value));
            block.body.push(InstrTyped::SetAttrTyped(field_store));
            stats.inserted_field_stores += 1;
        }
        stats.materialized_objects += 1;
    }
    stats
}

pub fn materialize_typed_virtual_store_boundaries_with_plan(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    plan: &TypedVirtualizationPlan,
) -> TypedVirtualStoreMaterializationStats {
    let Some(field_states) = plan.field_states.as_ref() else {
        return TypedVirtualStoreMaterializationStats::default();
    };
    let labels = typed_block_indices_by_label(function);
    let boundaries = plan
        .materialization_boundaries
        .iter()
        .filter_map(|boundary| match boundary.location {
            TypedVirtualBoundaryLocation::BodyInstr { block, instr_index }
                if boundary.kind == TypedVirtualBoundaryKind::EscapingStore =>
            {
                Some((boundary.source, (block, instr_index)))
            }
            TypedVirtualBoundaryLocation::BodyInstr { .. }
            | TypedVirtualBoundaryLocation::Term { .. }
            | TypedVirtualBoundaryLocation::ExceptionEdge { .. } => None,
        })
        .collect::<HashMap<_, _>>();
    let mut insertions = Vec::<(
        TypedVirtualObjectPlan,
        BlockLabel,
        TypedVirtualMaterializationRecipe,
        Vec<ResolvedName>,
    )>::new();
    for object in &plan.materializing_objects {
        let Some((block_label, planned_boundary_index)) = boundaries.get(&object.source).copied()
        else {
            continue;
        };
        let Some(recipe) = object.materialization_recipe.clone() else {
            continue;
        };
        let Some(block_index) = labels.get(&block_label).copied() else {
            continue;
        };
        let boundary_index = function.blocks[block_index].body.iter().position(|instr| {
            matches!(
                instr,
                InstrTyped::Store(store)
                    if typed_expr_loads_resolved_name(store.value.as_ref(), &object.root)
                        && !typed_resolved_name_is_virtual_constructor(&store.name, object)
            )
        });
        let Some(_boundary_index) = boundary_index else {
            continue;
        };
        let Some(state) = field_states.body_before_instr.get(&TypedVirtualBodyInstr {
            block: block_label,
            instr_index: planned_boundary_index,
        }) else {
            continue;
        };
        let field_values = object
            .field_bindings
            .fields
            .iter()
            .map(|field| {
                state
                    .fields
                    .get(&TypedVirtualFieldRef {
                        object: object.object_id,
                        field_name: field.field_name.clone(),
                    })
                    .cloned()
            })
            .collect::<Option<Vec<_>>>();
        let Some(field_values) = field_values else {
            continue;
        };
        insertions.push((object.clone(), block_label, recipe, field_values));
    }

    let eligible_plans = insertions
        .iter()
        .map(|(object, _, _, _)| object.clone())
        .collect::<Vec<_>>();
    let mut stats = TypedVirtualStoreMaterializationStats {
        virtualization: virtualize_typed_hot_constructor_plans(
            function,
            module_constants,
            &eligible_plans,
        ),
        ..TypedVirtualStoreMaterializationStats::default()
    };
    for (object, block_label, recipe, field_values) in insertions {
        let Some(block_index) = labels.get(&block_label).copied() else {
            continue;
        };
        let boundary_index = function.blocks[block_index].body.iter().position(|instr| {
            matches!(
                instr,
                InstrTyped::Store(store)
                    if typed_expr_loads_resolved_name(store.value.as_ref(), &object.root)
                        && !typed_resolved_name_is_virtual_constructor(&store.name, &object)
            )
        });
        let Some(boundary_index) = boundary_index else {
            continue;
        };
        let mut materialization = Vec::with_capacity(recipe.field_stores.len() + 1);
        materialization.push(recipe.allocation);
        stats.inserted_allocations += 1;
        for (mut field_store, field_value) in recipe.field_stores.into_iter().zip(field_values) {
            field_store.value = Box::new(typed_load_temp(&object.root));
            field_store.replacement = Box::new(typed_load_temp(&field_value));
            materialization.push(InstrTyped::SetAttrTyped(field_store));
            stats.inserted_field_stores += 1;
        }
        function.blocks[block_index]
            .body
            .splice(boundary_index..boundary_index, materialization);
        stats.materialized_objects += 1;
    }
    stats
}

pub fn materialize_typed_virtual_body_boundaries_with_plan(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    plan: &TypedVirtualizationPlan,
) -> TypedVirtualBodyMaterializationStats {
    let Some(field_states) = plan.field_states.as_ref() else {
        return TypedVirtualBodyMaterializationStats::default();
    };
    let labels = typed_block_indices_by_label(function);
    let boundaries = plan
        .materialization_boundaries
        .iter()
        .filter_map(|boundary| match boundary.location {
            TypedVirtualBoundaryLocation::BodyInstr { block, instr_index }
                if matches!(
                    boundary.kind,
                    TypedVirtualBoundaryKind::UnsupportedBodyUse
                        | TypedVirtualBoundaryKind::DeoptResumeUse
                ) =>
            {
                Some((boundary.source, (block, instr_index, boundary.kind)))
            }
            TypedVirtualBoundaryLocation::BodyInstr { .. }
            | TypedVirtualBoundaryLocation::Term { .. }
            | TypedVirtualBoundaryLocation::ExceptionEdge { .. } => None,
        })
        .collect::<HashMap<_, _>>();
    let mut insertions = Vec::<(
        TypedVirtualObjectPlan,
        BlockLabel,
        usize,
        TypedVirtualMaterializationRecipe,
        Vec<ResolvedName>,
    )>::new();
    for object in &plan.materializing_objects {
        let Some((block_label, planned_boundary_index, boundary_kind)) =
            boundaries.get(&object.source).copied()
        else {
            continue;
        };
        let Some(recipe) = object.materialization_recipe.clone() else {
            continue;
        };
        let Some(block_index) = labels.get(&block_label).copied() else {
            continue;
        };
        let boundary_index = function.blocks[block_index].body.iter().position(|instr| {
            typed_virtual_body_instr_matches_materialization_boundary(
                instr,
                module_constants,
                object,
                boundary_kind,
            )
        });
        let Some(boundary_index) = boundary_index else {
            continue;
        };
        let Some(state) = field_states.body_before_instr.get(&TypedVirtualBodyInstr {
            block: block_label,
            instr_index: planned_boundary_index,
        }) else {
            continue;
        };
        let field_values = object
            .field_bindings
            .fields
            .iter()
            .map(|field| {
                state
                    .fields
                    .get(&TypedVirtualFieldRef {
                        object: object.object_id,
                        field_name: field.field_name.clone(),
                    })
                    .cloned()
            })
            .collect::<Option<Vec<_>>>();
        let Some(field_values) = field_values else {
            continue;
        };
        insertions.push((
            object.clone(),
            block_label,
            boundary_index,
            recipe,
            field_values,
        ));
    }

    let eligible_plans = insertions
        .iter()
        .map(|(object, _, _, _, _)| object.clone())
        .collect::<Vec<_>>();
    let mut stats = TypedVirtualBodyMaterializationStats {
        virtualization: virtualize_typed_hot_constructor_plans(
            function,
            module_constants,
            &eligible_plans,
        ),
        ..TypedVirtualBodyMaterializationStats::default()
    };
    for (object, block_label, boundary_index, recipe, field_values) in insertions {
        let Some(block_index) = labels.get(&block_label).copied() else {
            continue;
        };
        let mut materialization = Vec::with_capacity(recipe.field_stores.len() + 1);
        materialization.push(recipe.allocation);
        stats.inserted_allocations += 1;
        for (mut field_store, field_value) in recipe.field_stores.into_iter().zip(field_values) {
            field_store.value = Box::new(typed_load_temp(&object.root));
            field_store.replacement = Box::new(typed_load_temp(&field_value));
            materialization.push(InstrTyped::SetAttrTyped(field_store));
            stats.inserted_field_stores += 1;
        }
        function.blocks[block_index]
            .body
            .splice(boundary_index..boundary_index, materialization);
        stats.materialized_objects += 1;
    }
    stats
}

fn typed_virtual_body_instr_matches_materialization_boundary(
    instr: &InstrTyped,
    module_constants: &[ConstantExpr],
    object: &TypedVirtualObjectPlan,
    kind: TypedVirtualBoundaryKind,
) -> bool {
    if kind == TypedVirtualBoundaryKind::DeoptResumeUse {
        return typed_expr_has_guard_miss_deopt(instr);
    }
    if !typed_expr_contains_resolved_name_load(instr, &object.root) {
        return false;
    }
    match instr {
        InstrTyped::Store(store)
            if store.name.local_location().is_some()
                && typed_expr_is_virtual_constructor_load(store.value.as_ref(), object) =>
        {
            false
        }
        InstrTyped::Del(del) if typed_resolved_name_is_virtual_constructor(&del.name, object) => {
            false
        }
        InstrTyped::SetAttrTyped(op)
            if typed_virtual_constructor_field_store(
                op,
                module_constants,
                &object.field_bindings,
                object,
            ) =>
        {
            false
        }
        _ => true,
    }
}

fn typed_expr_has_guard_miss_deopt(expr: &InstrTyped) -> bool {
    struct Finder {
        found: bool,
    }

    impl Visit<InstrTyped> for Finder {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.found {
                return;
            }
            if expr.guard_miss_deopt_enabled() {
                self.found = true;
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder { found: false };
    finder.visit_instr(expr);
    finder.found
}

pub(super) fn virtualize_typed_hot_constructor_plans(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    plans: &[TypedVirtualObjectPlan],
) -> TypedVirtualConstructorStats {
    let mut stats = TypedVirtualConstructorStats {
        planned_objects: plans.len(),
        ..TypedVirtualConstructorStats::default()
    };
    if plans.is_empty() {
        return stats;
    }
    let param_removals = typed_virtual_constructor_param_removals(function, plans);
    for block in &mut function.blocks {
        if let Some(remove) = param_removals.get(&block.label) {
            let before = block.params.len();
            block.params = block
                .params
                .iter()
                .enumerate()
                .filter_map(|(index, param)| (!remove.contains(&index)).then_some(param.clone()))
                .collect();
            stats.removed_block_params += before.saturating_sub(block.params.len());
        }
    }
    for block in &mut function.blocks {
        rewrite_typed_virtual_constructor_edges(block, &param_removals, &mut stats);
    }
    for block in &mut function.blocks {
        rewrite_typed_virtual_constructor_block(block, module_constants, plans, &mut stats);
    }
    stats
}

fn typed_virtual_constructor_materialization(
    instr: &InstrTyped,
) -> Option<(InstrId, &ResolvedName)> {
    let InstrTyped::Store(store) = instr else {
        return None;
    };
    let InstrTyped::CallTyped(call) = store.value.as_ref() else {
        return None;
    };
    let instr_id = call.try_semantic_instr_id()?;
    Some((instr_id, &store.name))
}

fn typed_virtual_materialization_recipe(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    bindings: &TypedConstructorFieldBindings,
    plan: &TypedVirtualObjectPlan,
) -> Option<TypedVirtualMaterializationRecipe> {
    let block = function
        .blocks
        .iter()
        .find(|block| block.label == plan.materialization_block)?;
    let allocation = block.body.get(plan.materialization_index)?.clone();
    let InstrTyped::Store(store) = &allocation else {
        return None;
    };
    let InstrTyped::CallTyped(call) = store.value.as_ref() else {
        return None;
    };
    if call.extra.constructor_init_plan()?.source
        != TypedConstructorInitPlanSource::InlinedConstructorEntryWithInlinedInitBody
    {
        return None;
    }

    let mut field_stores = HashMap::<String, TypedSetAttr<InstrTyped>>::new();
    for block in &function.blocks {
        for instr in &block.body {
            let InstrTyped::SetAttrTyped(op) = instr else {
                continue;
            };
            if !typed_virtual_constructor_field_store(op, module_constants, bindings, plan) {
                continue;
            }
            let field_name = typed_constant_string(op.attr.as_ref(), module_constants)?;
            field_stores
                .entry(field_name.to_string())
                .or_insert_with(|| op.clone());
        }
    }
    let field_stores = bindings
        .fields
        .iter()
        .map(|field| field_stores.remove(&field.field_name))
        .collect::<Option<Vec<_>>>()?;
    Some(TypedVirtualMaterializationRecipe {
        allocation,
        field_stores,
    })
}

fn typed_virtual_materialization_recipe_inputs_bound_at_boundary(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    recipe: &TypedVirtualMaterializationRecipe,
    boundary: TypedVirtualMaterializationBoundary,
) -> bool {
    struct LocalLoadCollector {
        locations: HashSet<LocalLocation>,
    }

    impl Visit<InstrTyped> for LocalLoadCollector {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::Load(load) = expr
                && let Some(location) = load.name.local_location()
            {
                self.locations.insert(location);
            }
            expr.visit_children(self);
        }
    }

    let mut required = LocalLoadCollector {
        locations: HashSet::new(),
    };
    required.visit_instr(&recipe.allocation);
    if required.locations.is_empty() {
        return true;
    }

    let (block_label, prefix_len) = match boundary.location {
        TypedVirtualBoundaryLocation::BodyInstr { block, instr_index } => (block, instr_index),
        TypedVirtualBoundaryLocation::Term { block } => {
            let Some(block) = function
                .blocks
                .iter()
                .find(|candidate| candidate.label == block)
            else {
                return false;
            };
            (block.label, block.body.len())
        }
        TypedVirtualBoundaryLocation::ExceptionEdge { .. } => return false,
    };
    let Some(block) = function
        .blocks
        .iter()
        .find(|candidate| candidate.label == block_label)
    else {
        return false;
    };
    let mut bound = compute_typed_function_local_must_bound_ins(function)
        .remove(&block_label)
        .unwrap_or_default();
    for instr in block.body.iter().take(prefix_len) {
        match instr {
            InstrTyped::Store(store) => {
                if let Some(location) = store.name.local_location() {
                    bound.insert(location);
                }
            }
            InstrTyped::Del(del) => {
                if let Some(location) = del.name.local_location() {
                    bound.remove(&location);
                }
            }
            _ => {}
        }
    }
    required.locations.is_subset(&bound)
}

fn complete_typed_virtual_constructor_plan(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    bindings: &TypedConstructorFieldBindings,
    plan: &mut TypedVirtualObjectPlan,
) -> Result<(), TypedVirtualMaterializationBoundary> {
    propagate_typed_virtual_constructor_aliases(function, plan);
    for block in &function.blocks {
        if !typed_virtual_constructor_plan_covers_block(plan, block.label) {
            continue;
        }
        let start = if block.label == plan.materialization_block {
            plan.materialization_index + 1
        } else {
            0
        };
        for (instr_index, instr) in block.body.iter().enumerate().skip(start) {
            if let Err(kind) = scan_typed_virtual_constructor_instr(
                function,
                module_constants,
                bindings,
                plan,
                instr,
            ) {
                return Err(TypedVirtualMaterializationBoundary {
                    source: plan.source,
                    location: TypedVirtualBoundaryLocation::BodyInstr {
                        block: block.label,
                        instr_index,
                    },
                    kind,
                });
            }
        }
        if let Err(kind) =
            scan_typed_virtual_constructor_term(function, module_constants, bindings, plan, block)
        {
            return Err(TypedVirtualMaterializationBoundary {
                source: plan.source,
                location: TypedVirtualBoundaryLocation::Term { block: block.label },
                kind,
            });
        }
        if let Some(edge) = &block.exc_edge
            && let Err(kind) = scan_typed_virtual_constructor_edge(function, plan, edge)
        {
            return Err(TypedVirtualMaterializationBoundary {
                source: plan.source,
                location: TypedVirtualBoundaryLocation::ExceptionEdge {
                    block: block.label,
                    target: edge.target,
                },
                kind,
            });
        }
    }
    Ok(())
}

fn propagate_typed_virtual_constructor_aliases(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plan: &mut TypedVirtualObjectPlan,
) {
    loop {
        let before_locations = plan.virtual_locations.len();
        let before_names = plan.virtual_names.len();
        for block in &function.blocks {
            if !typed_virtual_constructor_plan_covers_block(plan, block.label) {
                continue;
            }
            let start = if block.label == plan.materialization_block {
                plan.materialization_index + 1
            } else {
                0
            };
            for instr in block.body.iter().skip(start) {
                if let InstrTyped::Store(store) = instr {
                    typed_virtual_constructor_alias_store(function, plan, store);
                }
            }
            if let BlockTerm::Jump(edge) = &block.term {
                propagate_typed_virtual_constructor_edge_aliases(function, plan, edge);
            }
            if let Some(edge) = &block.exc_edge {
                propagate_typed_virtual_constructor_edge_aliases(function, plan, edge);
            }
        }
        if plan.virtual_locations.len() == before_locations
            && plan.virtual_names.len() == before_names
        {
            break;
        }
    }
}

fn propagate_typed_virtual_constructor_edge_aliases(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plan: &mut TypedVirtualObjectPlan,
    edge: &BlockEdge,
) {
    if !plan.reachable_blocks.contains(&edge.target) {
        return;
    }
    let Some(target) = function
        .blocks
        .iter()
        .find(|block| block.label == edge.target)
    else {
        return;
    };
    for (index, arg) in edge.args.iter().enumerate() {
        let BlockArg::Name(name) = arg else {
            continue;
        };
        if !plan.virtual_names.contains(name) {
            continue;
        }
        let Some(param) = target.params.get(index) else {
            continue;
        };
        if param.role != BlockParamRole::Value {
            continue;
        }
        plan.virtual_names.insert(param.name.clone());
        if let Some(location) = typed_local_location_for_name(function, &param.name) {
            plan.virtual_locations.insert(location);
        }
    }
}

pub(super) fn typed_virtual_constructor_plan_covers_block(
    plan: &TypedVirtualObjectPlan,
    label: BlockLabel,
) -> bool {
    label == plan.materialization_block || plan.reachable_blocks.contains(&label)
}

fn scan_typed_virtual_constructor_instr(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    bindings: &TypedConstructorFieldBindings,
    plan: &mut TypedVirtualObjectPlan,
    instr: &InstrTyped,
) -> Result<(), TypedVirtualBoundaryKind> {
    if typed_expr_has_guard_miss_deopt(instr) {
        return Err(TypedVirtualBoundaryKind::DeoptResumeUse);
    }
    match instr {
        InstrTyped::Store(store) => {
            if typed_virtual_constructor_alias_store(function, plan, store) {
                return Ok(());
            }
            if typed_expr_uses_virtual_constructor_identity(
                store.value.as_ref(),
                module_constants,
                bindings,
                plan,
            ) {
                return Err(TypedVirtualBoundaryKind::EscapingStore);
            }
            if typed_resolved_name_is_virtual_constructor(&store.name, plan) {
                return Err(TypedVirtualBoundaryKind::VirtualNameRebind);
            }
            Ok(())
        }
        InstrTyped::Del(del) if typed_resolved_name_is_virtual_constructor(&del.name, plan) => {
            Ok(())
        }
        InstrTyped::SetAttrTyped(op)
            if typed_virtual_constructor_field_store(op, module_constants, bindings, plan) =>
        {
            if typed_expr_uses_virtual_constructor_identity(
                op.attr.as_ref(),
                module_constants,
                bindings,
                plan,
            ) || typed_expr_uses_virtual_constructor_identity(
                op.replacement.as_ref(),
                module_constants,
                bindings,
                plan,
            ) {
                return Err(TypedVirtualBoundaryKind::UnsupportedBodyUse);
            }
            Ok(())
        }
        _ => {
            if typed_expr_uses_virtual_constructor_identity(instr, module_constants, bindings, plan)
            {
                return Err(TypedVirtualBoundaryKind::UnsupportedBodyUse);
            }
            Ok(())
        }
    }
}

fn scan_typed_virtual_constructor_term(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    bindings: &TypedConstructorFieldBindings,
    plan: &mut TypedVirtualObjectPlan,
    block: &TypedBlock,
) -> Result<(), TypedVirtualBoundaryKind> {
    match &block.term {
        BlockTerm::IfTerm(if_term) => {
            if let InstrTyped::DirectCallGuardTest(guard) = &if_term.test
                && typed_expr_is_virtual_constructor_load(guard.value.as_ref(), plan)
            {
                let TypedDirectCallGuardTestKind::ExactTypeVersion { owner_type_ref, .. } =
                    &guard.kind
                else {
                    return Err(TypedVirtualBoundaryKind::UnsupportedGuard);
                };
                if plan
                    .assumed_owner_type
                    .as_ref()
                    .is_some_and(|existing| existing != owner_type_ref)
                {
                    return Err(TypedVirtualBoundaryKind::UnsupportedGuard);
                }
                plan.assumed_owner_type = Some(owner_type_ref.clone());
                plan.guard_blocks.insert(block.label);
                return Ok(());
            }
            if typed_expr_uses_virtual_constructor_identity(
                &if_term.test,
                module_constants,
                bindings,
                plan,
            ) {
                return Err(TypedVirtualBoundaryKind::BranchTestUse);
            }
            Ok(())
        }
        BlockTerm::Jump(edge) => scan_typed_virtual_constructor_edge(function, plan, edge),
        BlockTerm::BranchTable(branch) => {
            if typed_expr_uses_virtual_constructor_identity(
                &branch.index,
                module_constants,
                bindings,
                plan,
            ) {
                return Err(TypedVirtualBoundaryKind::BranchTestUse);
            }
            Ok(())
        }
        BlockTerm::Raise(raise) => {
            if raise.exc.as_ref().is_some_and(|exc| {
                typed_expr_uses_virtual_constructor_identity(exc, module_constants, bindings, plan)
            }) {
                return Err(TypedVirtualBoundaryKind::RaiseUse);
            }
            Ok(())
        }
        BlockTerm::Return(value) => {
            if typed_expr_uses_virtual_constructor_identity(value, module_constants, bindings, plan)
            {
                return Err(TypedVirtualBoundaryKind::ReturnUse);
            }
            Ok(())
        }
    }
}

fn scan_typed_virtual_constructor_edge(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plan: &mut TypedVirtualObjectPlan,
    edge: &BlockEdge,
) -> Result<(), TypedVirtualBoundaryKind> {
    for arg in &edge.args {
        if let BlockArg::Name(name) = arg
            && plan.virtual_names.contains(name)
            && !plan.reachable_blocks.contains(&edge.target)
        {
            return Err(TypedVirtualBoundaryKind::EscapingEdge);
        }
    }
    let Some(target) = function
        .blocks
        .iter()
        .find(|block| block.label == edge.target)
    else {
        return Ok(());
    };
    for (index, arg) in edge.args.iter().enumerate() {
        let BlockArg::Name(name) = arg else {
            continue;
        };
        if !plan.virtual_names.contains(name) {
            continue;
        }
        let Some(param) = target.params.get(index) else {
            continue;
        };
        if param.role != BlockParamRole::Value {
            return Err(TypedVirtualBoundaryKind::UnsupportedEdgeParam);
        }
        plan.virtual_names.insert(param.name.clone());
        if let Some(location) = typed_local_location_for_name(function, &param.name) {
            plan.virtual_locations.insert(location);
        }
    }
    Ok(())
}

fn typed_virtual_constructor_alias_store(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plan: &mut TypedVirtualObjectPlan,
    store: &Store<InstrTyped>,
) -> bool {
    if !typed_expr_is_virtual_constructor_load(store.value.as_ref(), plan) {
        return false;
    }
    let Some(location) = store.name.local_location() else {
        return false;
    };
    plan.virtual_locations.insert(location);
    plan.virtual_names.insert(store.name.id_str().to_string());
    if let Some(location) = typed_local_location_for_name(function, store.name.id_str()) {
        plan.virtual_locations.insert(location);
    }
    true
}

fn typed_virtual_constructor_field_store(
    op: &TypedSetAttr<InstrTyped>,
    module_constants: &[ConstantExpr],
    bindings: &TypedConstructorFieldBindings,
    plan: &TypedVirtualObjectPlan,
) -> bool {
    if !typed_expr_is_virtual_constructor_load(op.value.as_ref(), plan)
        || !typed_attr_access_is_indexed_field(&op.access)
    {
        return false;
    }
    let Some(field_name) = typed_constant_string(op.attr.as_ref(), module_constants) else {
        return false;
    };
    bindings
        .fields
        .iter()
        .any(|field| field.field_name == field_name)
}

fn typed_virtual_constructor_field_load(
    op: &TypedGetAttr<InstrTyped>,
    module_constants: &[ConstantExpr],
    bindings: &TypedConstructorFieldBindings,
    plan: &TypedVirtualObjectPlan,
) -> bool {
    if !typed_expr_is_virtual_constructor_load(op.value.as_ref(), plan)
        || !typed_attr_access_is_indexed_field(&op.access)
    {
        return false;
    }
    let Some(field_name) = typed_constant_string(op.attr.as_ref(), module_constants) else {
        return false;
    };
    bindings
        .fields
        .iter()
        .any(|field| field.field_name == field_name)
}

fn typed_expr_is_virtual_constructor_load(
    expr: &InstrTyped,
    plan: &TypedVirtualObjectPlan,
) -> bool {
    let InstrTyped::Load(load) = expr else {
        return false;
    };
    typed_resolved_name_is_virtual_constructor(&load.name, plan)
}

fn typed_resolved_name_is_virtual_constructor(
    name: &ResolvedName,
    plan: &TypedVirtualObjectPlan,
) -> bool {
    name.local_location()
        .is_some_and(|location| plan.virtual_locations.contains(&location))
        || plan.virtual_names.contains(name.id_str())
}

fn typed_expr_uses_virtual_constructor_identity(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
    bindings: &TypedConstructorFieldBindings,
    plan: &TypedVirtualObjectPlan,
) -> bool {
    struct Finder<'a> {
        module_constants: &'a [ConstantExpr],
        bindings: &'a TypedConstructorFieldBindings,
        plan: &'a TypedVirtualObjectPlan,
        found: bool,
    }

    impl Visit<InstrTyped> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.found {
                return;
            }
            if let InstrTyped::GetAttrTyped(op) = expr
                && typed_virtual_constructor_field_load(
                    op,
                    self.module_constants,
                    self.bindings,
                    self.plan,
                )
            {
                self.visit_instr(op.attr.as_ref());
                return;
            }
            if typed_expr_is_virtual_constructor_load(expr, self.plan) {
                self.found = true;
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder {
        module_constants,
        bindings,
        plan,
        found: false,
    };
    finder.visit_instr(expr);
    finder.found
}

fn typed_local_location_for_name(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    name: &str,
) -> Option<LocalLocation> {
    let layout = function.storage_layout.as_ref()?;
    layout
        .stack_slots()
        .iter()
        .position(|slot_name| slot_name == name)
        .map(|slot| {
            LocalLocation(
                u32::try_from(slot).expect("stack slot index should fit in LocalLocation"),
            )
        })
}

fn typed_resolved_local_for_name(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    name: &str,
) -> Option<ResolvedName> {
    Some(ResolvedName {
        id: name.to_string().into(),
        location: NameLocation::Local(typed_local_location_for_name(function, name)?),
    })
}

fn typed_virtual_constructor_param_removals(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plans: &[TypedVirtualObjectPlan],
) -> HashMap<BlockLabel, HashSet<usize>> {
    let mut removals = HashMap::<BlockLabel, HashSet<usize>>::new();
    for block in &function.blocks {
        for plan in plans {
            if !plan.reachable_blocks.contains(&block.label) {
                continue;
            }
            for (index, param) in block.params.iter().enumerate() {
                if param.role == BlockParamRole::Value && plan.virtual_names.contains(&param.name) {
                    removals.entry(block.label).or_default().insert(index);
                }
            }
        }
    }
    removals
}

fn rewrite_typed_virtual_constructor_edges(
    block: &mut TypedBlock,
    param_removals: &HashMap<BlockLabel, HashSet<usize>>,
    stats: &mut TypedVirtualConstructorStats,
) {
    match &mut block.term {
        BlockTerm::Jump(edge) => {
            stats.removed_block_args += rewrite_typed_virtual_constructor_edge(edge, param_removals)
        }
        BlockTerm::IfTerm(_)
        | BlockTerm::BranchTable(_)
        | BlockTerm::Raise(_)
        | BlockTerm::Return(_) => {}
    }
    if let Some(edge) = &mut block.exc_edge {
        stats.removed_block_args += rewrite_typed_virtual_constructor_edge(edge, param_removals);
    }
}

fn rewrite_typed_virtual_constructor_edge(
    edge: &mut BlockEdge,
    param_removals: &HashMap<BlockLabel, HashSet<usize>>,
) -> usize {
    let Some(remove) = param_removals.get(&edge.target) else {
        return 0;
    };
    let before = edge.args.len();
    edge.args = edge
        .args
        .iter()
        .enumerate()
        .filter_map(|(index, arg)| (!remove.contains(&index)).then_some(arg.clone()))
        .collect();
    before.saturating_sub(edge.args.len())
}

fn rewrite_typed_virtual_constructor_block(
    block: &mut TypedBlock,
    module_constants: &[ConstantExpr],
    plans: &[TypedVirtualObjectPlan],
    stats: &mut TypedVirtualConstructorStats,
) {
    let old_body = std::mem::take(&mut block.body);
    let mut new_body = Vec::with_capacity(old_body.len());
    for (instr_index, instr) in old_body.into_iter().enumerate() {
        if typed_virtual_constructor_should_remove_instr(
            block.label,
            instr_index,
            &instr,
            module_constants,
            plans,
            stats,
        ) {
            continue;
        }
        new_body.push(instr);
    }
    block.body = new_body;
    let guard_then_label = match &block.term {
        BlockTerm::IfTerm(if_term)
            if plans
                .iter()
                .any(|plan| plan.guard_blocks.contains(&block.label)) =>
        {
            Some(if_term.then_label)
        }
        _ => None,
    };
    if let Some(then_label) = guard_then_label {
        block.term = BlockTerm::Jump(BlockEdge::new(then_label));
        stats.removed_guards += 1;
    }
}

fn typed_virtual_constructor_should_remove_instr(
    label: BlockLabel,
    instr_index: usize,
    instr: &InstrTyped,
    module_constants: &[ConstantExpr],
    plans: &[TypedVirtualObjectPlan],
    stats: &mut TypedVirtualConstructorStats,
) -> bool {
    for plan in plans {
        if label == plan.materialization_block
            && instr_index == plan.materialization_index
            && typed_virtual_constructor_materialization(instr)
                .is_some_and(|(source, _)| source == plan.source)
        {
            stats.removed_materializations += 1;
            return true;
        }
        if !typed_virtual_constructor_plan_covers_block(plan, label)
            || (label == plan.materialization_block && instr_index <= plan.materialization_index)
        {
            continue;
        }
        match instr {
            InstrTyped::Store(store)
                if store.name.local_location().is_some()
                    && typed_expr_is_virtual_constructor_load(store.value.as_ref(), plan) =>
            {
                stats.removed_alias_stores += 1;
                return true;
            }
            InstrTyped::Del(del) if typed_resolved_name_is_virtual_constructor(&del.name, plan) => {
                stats.removed_dels += 1;
                return true;
            }
            InstrTyped::SetAttrTyped(op)
                if typed_virtual_constructor_field_store(
                    op,
                    module_constants,
                    &plan.field_bindings,
                    plan,
                ) =>
            {
                stats.removed_field_stores += 1;
                return true;
            }
            _ => {}
        }
    }
    false
}

fn typed_field_scalar_in_state_for_block(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block: &TypedBlock,
    entry_label: Option<BlockLabel>,
    predecessors: &HashMap<BlockLabel, Vec<TypedScalarPredecessorEdge>>,
    labels: &HashMap<BlockLabel, usize>,
    out_states: &[Option<TypedVirtualLoweringState>],
) -> Option<TypedVirtualLoweringState> {
    let Some(predecessors) = predecessors.get(&block.label) else {
        return (Some(block.label) == entry_label).then(TypedVirtualLoweringState::default);
    };
    let computed = predecessors
        .iter()
        .filter_map(|edge| {
            labels
                .get(&edge.from)
                .and_then(|index| out_states.get(*index))
                .and_then(|state| state.as_ref())
                .map(|state| {
                    remap_typed_field_scalar_state_for_edge(
                        function,
                        block,
                        edge.explicit_args.as_deref(),
                        state,
                    )
                })
        })
        .collect::<Vec<_>>();
    if computed.is_empty() {
        None
    } else {
        Some(merge_typed_field_scalar_states(computed.iter()))
    }
}

fn allocate_typed_constructor_field_scalar_slots(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    constructor_field_bindings: &mut HashMap<InstrId, TypedConstructorFieldBindings>,
) -> usize {
    if function.storage_layout.is_none() {
        return 0;
    }
    let mut allocated = 0;
    for bindings in constructor_field_bindings.values_mut() {
        for field in &mut bindings.fields {
            if field.scalar.is_some() {
                continue;
            }
            let Ok(temp) = try_allocate_typed_stack_temp(function, "_dp_typed_scalar_field") else {
                continue;
            };
            field.scalar = Some(temp.resolved_name());
            allocated += 1;
        }
    }
    allocated
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
struct TypedVirtualFieldBlockParamStats {
    inserted_block_params: usize,
    inserted_block_args: usize,
}

#[derive(Debug, Clone)]
struct TypedVirtualFieldBlockParamCandidate {
    target: BlockLabel,
    incoming: Vec<(BlockLabel, ResolvedName)>,
}

fn synthesize_typed_virtual_field_block_params(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    analysis: &TypedVirtualLoweringAnalysis,
    removable_objects: &HashSet<TypedVirtualObjectId>,
) -> TypedVirtualFieldBlockParamStats {
    let predecessors = typed_scalar_block_predecessor_edges(function);
    let mut candidates = Vec::new();
    for block in &function.blocks {
        let Some(incoming_edges) = predecessors.get(&block.label) else {
            continue;
        };
        if incoming_edges.len() < 2
            || incoming_edges.iter().any(|edge| {
                edge.explicit_args
                    .as_ref()
                    .is_none_or(|args| args.len() != block.params.len())
            })
        {
            continue;
        }
        let incoming_states = incoming_edges
            .iter()
            .map(|edge| {
                analysis.block_out.get(&edge.from).map(|state| {
                    remap_typed_field_scalar_state_for_edge(
                        function,
                        block,
                        edge.explicit_args.as_deref(),
                        state,
                    )
                })
            })
            .collect::<Option<Vec<_>>>();
        let Some(incoming_states) = incoming_states else {
            continue;
        };
        let Some(first_state) = incoming_states.first() else {
            continue;
        };
        let mut fields = first_state
            .virtual_state
            .fields
            .keys()
            .filter(|field| removable_objects.contains(&field.object))
            .filter(|field| {
                incoming_states
                    .iter()
                    .skip(1)
                    .all(|state| state.virtual_state.fields.contains_key(*field))
            })
            .filter(|field| {
                let Some(first_value) = first_state.virtual_state.fields.get(*field) else {
                    return false;
                };
                incoming_states.iter().skip(1).any(|state| {
                    state
                        .virtual_state
                        .fields
                        .get(*field)
                        .is_some_and(|incoming| {
                            state.resolve_scalar_name(incoming)
                                != first_state.resolve_scalar_name(first_value)
                        })
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        fields.sort_by(|left, right| {
            left.object
                .0
                .cmp(&right.object.0)
                .then_with(|| left.field_name.cmp(&right.field_name))
        });
        for field in fields {
            let incoming = incoming_edges
                .iter()
                .map(|edge| {
                    analysis
                        .block_out
                        .get(&edge.from)
                        .and_then(|state| state.virtual_state.fields.get(&field))
                        .cloned()
                        .map(|value| (edge.from, value))
                })
                .collect::<Option<Vec<_>>>();
            let Some(incoming) = incoming else {
                continue;
            };
            candidates.push(TypedVirtualFieldBlockParamCandidate {
                target: block.label,
                incoming,
            });
        }
    }

    let mut stats = TypedVirtualFieldBlockParamStats::default();
    for candidate in candidates {
        let Ok(param) = try_allocate_typed_stack_temp(function, "_dp_vfield") else {
            continue;
        };
        let Some(target_index) = function
            .blocks
            .iter()
            .position(|block| block.label == candidate.target)
        else {
            continue;
        };
        function.blocks[target_index].params.push(BlockParam {
            name: param.name.clone(),
            role: BlockParamRole::Value,
        });
        stats.inserted_block_params += 1;
        for (from, value) in candidate.incoming {
            if append_typed_jump_edge_arg(
                function,
                from,
                candidate.target,
                BlockArg::Name(value.id_str().to_string()),
            ) {
                stats.inserted_block_args += 1;
            }
        }
    }
    stats
}

fn split_typed_virtual_field_block_param_edges(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    analysis: &TypedVirtualLoweringAnalysis,
    removable_objects: &HashSet<TypedVirtualObjectId>,
) -> usize {
    let predecessors = typed_scalar_block_predecessor_edges(function);
    let mut edges_to_split = HashSet::<(BlockLabel, BlockLabel)>::new();
    for block in &function.blocks {
        let Some(incoming_edges) = predecessors.get(&block.label) else {
            continue;
        };
        if incoming_edges.len() < 2 {
            continue;
        }
        let incoming_states = incoming_edges
            .iter()
            .map(|edge| {
                analysis.block_out.get(&edge.from).map(|state| {
                    remap_typed_field_scalar_state_for_edge(
                        function,
                        block,
                        edge.explicit_args.as_deref(),
                        state,
                    )
                })
            })
            .collect::<Option<Vec<_>>>();
        let Some(incoming_states) = incoming_states else {
            continue;
        };
        let Some(first_state) = incoming_states.first() else {
            continue;
        };
        let needs_virtual_field_param =
            first_state
                .virtual_state
                .fields
                .iter()
                .any(|(field, first_value)| {
                    removable_objects.contains(&field.object)
                        && incoming_states
                            .iter()
                            .skip(1)
                            .all(|state| state.virtual_state.fields.contains_key(field))
                        && incoming_states.iter().skip(1).any(|state| {
                            state
                                .virtual_state
                                .fields
                                .get(field)
                                .is_some_and(|incoming| {
                                    state.resolve_scalar_name(incoming)
                                        != first_state.resolve_scalar_name(first_value)
                                })
                        })
                });
        if !needs_virtual_field_param {
            continue;
        }
        for edge in incoming_edges {
            if edge.explicit_args.is_none() {
                edges_to_split.insert((edge.from, block.label));
            }
        }
    }

    let mut inserted = 0;
    for (from, target) in edges_to_split {
        let trampoline = function.name_gen.next_block_name();
        let Some(source) = function.blocks.iter_mut().find(|block| block.label == from) else {
            continue;
        };
        if !source.term.replace_target(target, trampoline) {
            continue;
        }
        function.blocks.push(Block::new_with_extra(
            trampoline,
            Vec::new(),
            BlockTerm::Jump(BlockEdge::new(target)),
            Vec::new(),
            None,
            TypedBlockExtra::default(),
        ));
        inserted += 1;
    }
    inserted
}

fn refresh_typed_virtual_field_block_param_args(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    analysis: &TypedVirtualLoweringAnalysis,
) {
    let blocks = function.blocks.clone();
    for block in blocks {
        for (param_index, param) in block.params.iter().enumerate() {
            if param.role != BlockParamRole::Value || !param.name.contains("_dp_vfield") {
                continue;
            }
            let Some(target_name) = typed_resolved_local_for_name(function, &param.name) else {
                continue;
            };
            let field = analysis.block_in.get(&block.label).and_then(|state| {
                state
                    .virtual_state
                    .fields
                    .iter()
                    .find_map(|(field, value)| (value == &target_name).then_some(field.clone()))
            });
            let Some(field) = field else {
                continue;
            };
            let predecessors = typed_scalar_block_predecessor_edges(function);
            let Some(incoming_edges) = predecessors.get(&block.label) else {
                continue;
            };
            for edge in incoming_edges {
                let Some(value) = analysis
                    .block_out
                    .get(&edge.from)
                    .and_then(|state| state.virtual_state.fields.get(&field))
                else {
                    continue;
                };
                set_typed_jump_edge_arg(
                    function,
                    edge.from,
                    block.label,
                    param_index,
                    BlockArg::Name(value.id_str().to_string()),
                );
            }
        }
    }
}

fn append_typed_jump_edge_arg(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    from: BlockLabel,
    target: BlockLabel,
    arg: BlockArg,
) -> bool {
    let Some(block) = function.blocks.iter_mut().find(|block| block.label == from) else {
        return false;
    };
    let BlockTerm::Jump(edge) = &mut block.term else {
        return false;
    };
    if edge.target != target {
        return false;
    }
    edge.args.push(arg);
    true
}

fn set_typed_jump_edge_arg(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    from: BlockLabel,
    target: BlockLabel,
    index: usize,
    arg: BlockArg,
) -> bool {
    let Some(block) = function.blocks.iter_mut().find(|block| block.label == from) else {
        return false;
    };
    let BlockTerm::Jump(edge) = &mut block.term else {
        return false;
    };
    if edge.target != target || edge.args.len() <= index {
        return false;
    }
    edge.args[index] = arg;
    true
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct TypedVirtualLoweringState {
    virtual_state: TypedVirtualState,
    value_aliases: HashMap<LocalLocation, ResolvedName>,
    field_scalars: HashMap<TypedVirtualFieldRef, ResolvedName>,
}

impl TypedVirtualLoweringState {
    pub(super) fn object_for_location(
        &self,
        location: LocalLocation,
    ) -> Option<TypedVirtualObjectId> {
        self.virtual_state.aliases.get(&location).copied()
    }

    pub(super) fn rebind_local(&mut self, location: LocalLocation) {
        self.value_aliases.remove(&location);
        self.value_aliases.retain(|_, value| {
            value
                .local_location()
                .is_none_or(|source| source != location)
        });
        self.virtual_state.fields.retain(|_, value| {
            value
                .local_location()
                .is_none_or(|source| source != location)
        });
        self.virtual_state.aliases.remove(&location);
    }

    pub(super) fn set_alias(&mut self, location: LocalLocation, object: TypedVirtualObjectId) {
        self.rebind_local(location);
        self.virtual_state.aliases.insert(location, object);
    }

    fn set_value_alias(&mut self, location: LocalLocation, value: &ResolvedName) {
        self.rebind_local(location);
        let value = self.resolve_scalar_name(value);
        self.value_aliases.insert(location, value);
    }

    fn resolve_scalar_name(&self, value: &ResolvedName) -> ResolvedName {
        let Some(mut location) = value.local_location() else {
            return value.clone();
        };
        let mut resolved = value.clone();
        let mut seen = HashSet::new();
        while seen.insert(location) {
            let Some(mapped) = self.value_aliases.get(&location) else {
                break;
            };
            resolved = mapped.clone();
            let Some(mapped_location) = mapped.local_location() else {
                return mapped.clone();
            };
            location = mapped_location;
        }
        resolved
    }

    pub(super) fn seed_object(
        &mut self,
        object: TypedVirtualObjectId,
        location: LocalLocation,
        bindings: &TypedConstructorFieldBindings,
    ) {
        self.rebind_local(location);
        self.virtual_state.aliases.insert(location, object);
        for field in &bindings.fields {
            let value = field
                .scalar
                .as_ref()
                .cloned()
                .unwrap_or_else(|| self.resolve_scalar_name(&field.value));
            let field_ref = TypedVirtualFieldRef {
                object,
                field_name: field.field_name.clone(),
            };
            self.virtual_state.fields.insert(field_ref.clone(), value);
            if let Some(scalar) = &field.scalar {
                self.field_scalars.insert(field_ref, scalar.clone());
            }
        }
    }

    pub(super) fn field_value(
        &self,
        object: TypedVirtualObjectId,
        field_name: &str,
    ) -> Option<&ResolvedName> {
        self.virtual_state.fields.get(&TypedVirtualFieldRef {
            object,
            field_name: field_name.to_string(),
        })
    }

    pub(super) fn field_scalar(
        &self,
        object: TypedVirtualObjectId,
        field_name: &str,
    ) -> Option<&ResolvedName> {
        self.field_scalars.get(&TypedVirtualFieldRef {
            object,
            field_name: field_name.to_string(),
        })
    }

    fn set_field(&mut self, object: TypedVirtualObjectId, field_name: String, value: ResolvedName) {
        let value = self.resolve_scalar_name(&value);
        self.virtual_state
            .fields
            .insert(TypedVirtualFieldRef { object, field_name }, value);
    }

    fn invalidate_field(&mut self, object: TypedVirtualObjectId, field_name: &str) {
        self.virtual_state.fields.remove(&TypedVirtualFieldRef {
            object,
            field_name: field_name.to_string(),
        });
    }

    fn invalidate_object(&mut self, object: TypedVirtualObjectId) {
        self.virtual_state
            .aliases
            .retain(|_, mapped_object| *mapped_object != object);
        self.virtual_state
            .fields
            .retain(|field, _| field.object != object);
        self.field_scalars.retain(|field, _| field.object != object);
    }

    fn invalidate_objects(&mut self, objects: HashSet<TypedVirtualObjectId>) {
        for object in objects {
            self.invalidate_object(object);
        }
    }

    fn remap_local(&mut self, source: &ResolvedName, target: &ResolvedName) {
        let Some(source_location) = source.local_location() else {
            return;
        };
        let Some(target_location) = target.local_location() else {
            return;
        };
        if let Some(object) = self.virtual_state.aliases.get(&source_location).copied() {
            self.virtual_state.aliases.insert(target_location, object);
        }
        for value in self.value_aliases.values_mut() {
            if value == source {
                *value = target.clone();
            }
        }
        for value in self.virtual_state.fields.values_mut() {
            if value == source {
                *value = target.clone();
            }
        }
    }
}

fn merge_typed_field_scalar_states<'a>(
    mut states: impl Iterator<Item = &'a TypedVirtualLoweringState>,
) -> TypedVirtualLoweringState {
    let Some(first) = states.next() else {
        return TypedVirtualLoweringState::default();
    };
    let rest = states.collect::<Vec<_>>();
    let mut merged = first.clone();
    merged.virtual_state.aliases.retain(|location, root| {
        rest.iter()
            .all(|state| state.virtual_state.aliases.get(location) == Some(root))
    });
    merged.value_aliases.retain(|location, value| {
        rest.iter()
            .all(|state| state.value_aliases.get(location) == Some(value))
    });
    merged.field_scalars.retain(|field, value| {
        rest.iter()
            .all(|state| state.field_scalars.get(field) == Some(value))
    });
    merged.virtual_state.fields.retain(|field, value| {
        let resolved = first.resolve_scalar_name(value);
        if !rest.iter().all(|state| {
            state
                .virtual_state
                .fields
                .get(field)
                .is_some_and(|incoming| state.resolve_scalar_name(incoming) == resolved)
        }) {
            return false;
        }
        *value = resolved;
        true
    });
    merged
}

#[derive(Clone, Debug)]
struct TypedScalarPredecessorEdge {
    from: BlockLabel,
    explicit_args: Option<Vec<BlockArg>>,
}

fn typed_scalar_block_predecessor_edges(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<BlockLabel, Vec<TypedScalarPredecessorEdge>> {
    let mut predecessors = HashMap::<BlockLabel, Vec<TypedScalarPredecessorEdge>>::new();
    for block in &function.blocks {
        match &block.term {
            BlockTerm::Jump(edge) => {
                predecessors
                    .entry(edge.target)
                    .or_default()
                    .push(TypedScalarPredecessorEdge {
                        from: block.label,
                        explicit_args: Some(edge.args.clone()),
                    })
            }
            BlockTerm::IfTerm(if_term) => {
                let targets = if matches!(if_term.test, InstrTyped::DirectCallGuardTest(_))
                    && if_term.test.guard_miss_deopt_enabled()
                {
                    vec![if_term.then_label]
                } else {
                    vec![if_term.then_label, if_term.else_label]
                };
                for target in targets {
                    predecessors
                        .entry(target)
                        .or_default()
                        .push(TypedScalarPredecessorEdge {
                            from: block.label,
                            explicit_args: None,
                        });
                }
            }
            BlockTerm::BranchTable(branch) => {
                for target in branch
                    .targets
                    .iter()
                    .copied()
                    .chain(std::iter::once(branch.default_label))
                {
                    predecessors
                        .entry(target)
                        .or_default()
                        .push(TypedScalarPredecessorEdge {
                            from: block.label,
                            explicit_args: None,
                        });
                }
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
        }
    }
    predecessors
}

fn remap_typed_field_scalar_state_for_edge(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    target: &TypedBlock,
    explicit_args: Option<&[BlockArg]>,
    state: &TypedVirtualLoweringState,
) -> TypedVirtualLoweringState {
    let Some(args) = explicit_args else {
        return state.clone();
    };
    let mut remapped = state.clone();
    for (param, arg) in target.params.iter().zip(args) {
        if param.role != BlockParamRole::Value {
            continue;
        }
        let BlockArg::Name(source_name) = arg else {
            continue;
        };
        let Some(source) = typed_resolved_local_for_name(function, source_name) else {
            continue;
        };
        let Some(target_name) = typed_resolved_local_for_name(function, &param.name) else {
            continue;
        };
        remapped.remap_local(&source, &target_name);
    }
    remapped
}

fn typed_scalar_term_successors(term: &BlockTerm<InstrTyped>) -> Vec<BlockLabel> {
    if let BlockTerm::IfTerm(if_term) = term
        && matches!(if_term.test, InstrTyped::DirectCallGuardTest(_))
        && if_term.test.guard_miss_deopt_enabled()
    {
        return vec![if_term.then_label];
    }
    typed_term_successors(term)
}

fn transfer_typed_field_scalar_block(
    block: &mut TypedBlock,
    state: &mut TypedVirtualLoweringState,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    transfer_typed_field_scalar_body(
        &mut block.body,
        state,
        module_constants,
        constructor_field_bindings,
        stats,
        rewrite,
    );
    transfer_typed_field_scalar_term(&mut block.term, state, module_constants, stats, rewrite);
}

fn transfer_typed_field_scalar_body(
    block_body: &mut Vec<InstrTyped>,
    state: &mut TypedVirtualLoweringState,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    let old_body = std::mem::take(block_body);
    let mut new_body = Vec::with_capacity(old_body.len());
    for mut instr in old_body {
        let mut inserted_before = Vec::new();
        let mut inserted_after = Vec::new();
        transfer_typed_field_scalar_instr(
            &mut instr,
            state,
            module_constants,
            constructor_field_bindings,
            stats,
            rewrite,
            &mut inserted_before,
            &mut inserted_after,
        );
        if rewrite {
            new_body.extend(inserted_before);
        }
        new_body.push(instr);
        if rewrite {
            new_body.extend(inserted_after);
        }
    }
    *block_body = new_body;
}

fn typed_field_scalar_body_snapshots(
    block: &TypedBlock,
    state: &mut TypedVirtualLoweringState,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
) -> Vec<TypedVirtualLoweringState> {
    let mut snapshots = Vec::with_capacity(block.body.len());
    let mut ignored_stats = TypedFieldScalarizationStats::default();
    for instr in &block.body {
        snapshots.push(state.clone());
        let mut instr = instr.clone();
        let mut inserted_before = Vec::new();
        let mut inserted_after = Vec::new();
        transfer_typed_field_scalar_instr(
            &mut instr,
            state,
            module_constants,
            constructor_field_bindings,
            &mut ignored_stats,
            false,
            &mut inserted_before,
            &mut inserted_after,
        );
    }
    snapshots
}

fn transfer_typed_field_scalar_instr(
    instr: &mut InstrTyped,
    state: &mut TypedVirtualLoweringState,
    module_constants: &[ConstantExpr],
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
    inserted_before: &mut Vec<InstrTyped>,
    inserted_after: &mut Vec<InstrTyped>,
) {
    match instr {
        InstrTyped::Store(store) => {
            if let Some((source, bindings)) =
                typed_constructor_field_bindings_for_store(store, constructor_field_bindings)
            {
                state
                    .invalidate_objects(typed_virtual_objects_in_expr(store.value.as_ref(), state));
                if let Some(location) = store.name.local_location() {
                    state.seed_object(TypedVirtualObjectId(source.index()), location, bindings);
                    if rewrite {
                        stats.seeded_objects += 1;
                        append_typed_constructor_field_scalar_stores(
                            inserted_after,
                            bindings,
                            state,
                            stats,
                        );
                    }
                }
                return;
            }
            rewrite_typed_field_scalar_expr(
                store.value.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            let Some(target) = store.name.local_location() else {
                return;
            };
            if let Some(source) = typed_instr_local_load_location(store.value.as_ref())
                && let Some(object) = state.object_for_location(source)
            {
                state.set_alias(target, object);
            } else if let InstrTyped::Load(source) = store.value.as_ref() {
                state.set_value_alias(target, &source.name);
            } else {
                state.rebind_local(target);
            }
        }
        InstrTyped::Del(del) => {
            if let Some(location) = del.name.local_location() {
                state.rebind_local(location);
            }
        }
        InstrTyped::SetAttrTyped(op) => rewrite_typed_field_scalar_setattr(
            op,
            state,
            module_constants,
            stats,
            rewrite,
            Some(inserted_before),
            Some(inserted_after),
        ),
        _ => rewrite_typed_field_scalar_expr(instr, state, module_constants, stats, rewrite),
    }
}

fn append_typed_constructor_field_scalar_stores(
    body: &mut Vec<InstrTyped>,
    bindings: &TypedConstructorFieldBindings,
    state: &TypedVirtualLoweringState,
    stats: &mut TypedFieldScalarizationStats,
) {
    for field in &bindings.fields {
        let Some(scalar) = field.scalar.as_ref() else {
            continue;
        };
        let source = state.resolve_scalar_name(&field.value);
        body.push(typed_store_temp(scalar.clone(), typed_load_temp(&source)));
        stats.inserted_scalar_stores += 1;
    }
}

fn rewrite_typed_field_scalar_setattr(
    op: &mut TypedSetAttr<InstrTyped>,
    state: &mut TypedVirtualLoweringState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
    inserted_before: Option<&mut Vec<InstrTyped>>,
    inserted_after: Option<&mut Vec<InstrTyped>>,
) {
    rewrite_typed_field_scalar_expr(
        op.replacement.as_mut(),
        state,
        module_constants,
        stats,
        rewrite,
    );
    state.invalidate_objects(typed_virtual_objects_in_expr(op.attr.as_ref(), state));
    let Some(receiver) = typed_instr_local_load_location(op.value.as_ref()) else {
        state.invalidate_objects(typed_virtual_objects_in_expr(op.value.as_ref(), state));
        return;
    };
    let Some(object) = state.object_for_location(receiver) else {
        return;
    };
    let Some(field_name) = typed_constant_string(op.attr.as_ref(), module_constants) else {
        state.invalidate_object(object);
        return;
    };
    if !typed_attr_access_is_indexed_field(&op.access) {
        state.invalidate_object(object);
        return;
    }
    if let Some(scalar) = state.field_scalar(object, field_name).cloned() {
        if let Some(replacement) = typed_scalar_field_replacement_name(op.replacement.as_ref()) {
            if rewrite && let Some(inserted_after) = inserted_after {
                inserted_after.push(typed_store_temp(
                    scalar.clone(),
                    typed_load_temp(replacement),
                ));
                stats.inserted_scalar_stores += 1;
            }
            state.set_field(object, field_name.to_string(), scalar);
        } else if typed_scalar_field_can_precompute_replacement(op.replacement.as_ref()) {
            if rewrite && let Some(inserted_before) = inserted_before {
                let replacement =
                    std::mem::replace(op.replacement.as_mut(), typed_load_temp(&scalar));
                inserted_before.push(typed_store_temp(scalar.clone(), replacement));
                stats.inserted_scalar_stores += 1;
            }
            state.set_field(object, field_name.to_string(), scalar);
        } else {
            state.invalidate_field(object, field_name);
        }
    } else if let Some(replacement) = typed_scalar_field_replacement_name(op.replacement.as_ref()) {
        state.set_field(object, field_name.to_string(), replacement.clone());
    } else {
        state.invalidate_field(object, field_name);
    }
}

fn typed_constructor_field_bindings_for_store<'a>(
    store: &Store<InstrTyped>,
    constructor_field_bindings: &'a HashMap<InstrId, TypedConstructorFieldBindings>,
) -> Option<(InstrId, &'a TypedConstructorFieldBindings)> {
    let InstrTyped::CallTyped(call) = store.value.as_ref() else {
        return None;
    };
    let instr_id = call.try_semantic_instr_id()?;
    constructor_field_bindings
        .get(&instr_id)
        .map(|bindings| (instr_id, bindings))
}

fn transfer_typed_field_scalar_term(
    term: &mut BlockTerm<InstrTyped>,
    state: &mut TypedVirtualLoweringState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    match term {
        BlockTerm::IfTerm(if_term) => {
            rewrite_typed_field_scalar_expr(
                &mut if_term.test,
                state,
                module_constants,
                stats,
                rewrite,
            );
            if !matches!(if_term.test, InstrTyped::DirectCallGuardTest(_)) {
                state.invalidate_objects(typed_virtual_objects_in_expr(&if_term.test, state));
            }
        }
        BlockTerm::BranchTable(branch) => {
            rewrite_typed_field_scalar_expr(
                &mut branch.index,
                state,
                module_constants,
                stats,
                rewrite,
            );
            state.invalidate_objects(typed_virtual_objects_in_expr(&branch.index, state));
        }
        BlockTerm::Raise(raise) => {
            if let Some(exc) = raise.exc.as_mut() {
                rewrite_typed_field_scalar_expr(exc, state, module_constants, stats, rewrite);
                state.invalidate_objects(typed_virtual_objects_in_expr(exc, state));
            }
        }
        BlockTerm::Return(value) => {
            rewrite_typed_field_scalar_expr(value, state, module_constants, stats, rewrite);
            state.invalidate_objects(typed_virtual_objects_in_expr(value, state));
        }
        BlockTerm::Jump(_) => {}
    }
}

fn rewrite_typed_field_scalar_expr(
    expr: &mut InstrTyped,
    state: &mut TypedVirtualLoweringState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    match expr {
        InstrTyped::Load(_) | InstrTyped::IncrementCounter(_) | InstrTyped::CellRef(_) => {}
        InstrTyped::GetAttrTyped(op) => {
            rewrite_typed_field_scalar_getattr(op, state, module_constants, stats, rewrite);
            if let Some(replacement) =
                typed_field_scalar_getattr_replacement(op, state, module_constants)
            {
                if rewrite {
                    stats.rewritten_loads += 1;
                }
                *expr = typed_load_temp(&replacement);
            }
        }
        InstrTyped::SetAttrTyped(op) => {
            rewrite_typed_field_scalar_setattr(
                op,
                state,
                module_constants,
                stats,
                rewrite,
                None,
                None,
            );
        }
        InstrTyped::DirectCallGuardTest(op) => {
            rewrite_typed_field_scalar_expr(
                op.value.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        _ => {
            rewrite_typed_field_scalar_children(expr, state, module_constants, stats, rewrite);
            state.invalidate_objects(typed_virtual_objects_in_expr(expr, state));
        }
    }
}

fn rewrite_typed_field_scalar_getattr(
    op: &mut TypedGetAttr<InstrTyped>,
    state: &mut TypedVirtualLoweringState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    if typed_instr_local_load_location(op.value.as_ref()).is_none() {
        rewrite_typed_field_scalar_expr(op.value.as_mut(), state, module_constants, stats, rewrite);
    }
    rewrite_typed_field_scalar_expr(op.attr.as_mut(), state, module_constants, stats, rewrite);
    if typed_field_scalar_getattr_replacement(op, state, module_constants).is_some() {
        return;
    }
    if let Some(receiver) = typed_instr_local_load_location(op.value.as_ref())
        && let Some(object) = state.object_for_location(receiver)
    {
        if typed_attr_access_is_indexed_field(&op.access)
            && let Some(field_name) = typed_constant_string(op.attr.as_ref(), module_constants)
        {
            state.invalidate_field(object, field_name);
            return;
        }
        state.invalidate_object(object);
    } else {
        state.invalidate_objects(typed_virtual_objects_in_expr(op.value.as_ref(), state));
    }
}

fn typed_field_scalar_getattr_replacement(
    op: &TypedGetAttr<InstrTyped>,
    state: &TypedVirtualLoweringState,
    module_constants: &[ConstantExpr],
) -> Option<ResolvedName> {
    if !typed_attr_access_is_indexed_field(&op.access) {
        return None;
    }
    let receiver = typed_instr_local_load_location(op.value.as_ref())?;
    let object = state.object_for_location(receiver)?;
    let field_name = typed_constant_string(op.attr.as_ref(), module_constants)?;
    state.field_value(object, field_name).cloned()
}

fn typed_attr_access_is_indexed_field(access: &TypedAttrAccessPlan) -> bool {
    matches!(access, TypedAttrAccessPlan::IndexedField { .. })
}

fn typed_scalar_field_replacement_name(expr: &InstrTyped) -> Option<&ResolvedName> {
    let InstrTyped::Load(load) = expr else {
        return None;
    };
    load.name.local_location()?;
    Some(&load.name)
}

fn typed_scalar_field_can_precompute_replacement(expr: &InstrTyped) -> bool {
    match expr {
        InstrTyped::Load(_) => true,
        InstrTyped::BinOp(op) => {
            typed_scalar_field_can_precompute_replacement(op.left.as_ref())
                && typed_scalar_field_can_precompute_replacement(op.right.as_ref())
        }
        InstrTyped::UnaryOp(op) => {
            typed_scalar_field_can_precompute_replacement(op.operand.as_ref())
        }
        InstrTyped::Tuple(op) => op
            .values
            .iter()
            .all(typed_scalar_field_can_precompute_replacement),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_ir_typed::lower_blockpy_module_to_typed;

    fn local_name(id: &str, location: u32) -> ResolvedName {
        ResolvedName {
            id: id.to_string().into(),
            location: NameLocation::Local(LocalLocation(location)),
        }
    }

    #[test]
    fn merge_preserves_field_state_when_paths_share_the_same_scalar_slot() {
        let object = TypedVirtualObjectId(7);
        let field = TypedVirtualFieldRef {
            object,
            field_name: "stop".to_string(),
        };
        let scalar = local_name("scalar_stop", 0);
        let mut left = TypedVirtualLoweringState::default();
        left.virtual_state
            .fields
            .insert(field.clone(), scalar.clone());
        left.field_scalars.insert(field.clone(), scalar.clone());

        let mut right = TypedVirtualLoweringState::default();
        let stop_alias = local_name("stop_alias", 1);
        right
            .virtual_state
            .fields
            .insert(field.clone(), stop_alias.clone());
        right.field_scalars.insert(field.clone(), scalar.clone());
        right.value_aliases.insert(
            stop_alias
                .local_location()
                .expect("test alias should have a local location"),
            scalar.clone(),
        );

        let merged = merge_typed_field_scalar_states([&left, &right].into_iter());

        assert_eq!(merged.field_scalars.get(&field), Some(&scalar));
        assert_eq!(merged.virtual_state.fields.get(&field), Some(&scalar));
    }

    #[test]
    fn merge_drops_field_state_when_incoming_aliases_do_not_resolve_together() {
        let object = TypedVirtualObjectId(7);
        let field = TypedVirtualFieldRef {
            object,
            field_name: "stop".to_string(),
        };
        let scalar = local_name("scalar_stop", 0);
        let mut left = TypedVirtualLoweringState::default();
        left.virtual_state
            .fields
            .insert(field.clone(), scalar.clone());
        left.field_scalars.insert(field.clone(), scalar.clone());

        let mut right = TypedVirtualLoweringState::default();
        right
            .virtual_state
            .fields
            .insert(field.clone(), local_name("unrelated_stop", 1));
        right.field_scalars.insert(field.clone(), scalar.clone());

        let merged = merge_typed_field_scalar_states([&left, &right].into_iter());

        assert_eq!(merged.field_scalars.get(&field), Some(&scalar));
        assert!(!merged.virtual_state.fields.contains_key(&field));
    }

    #[test]
    fn split_virtual_field_param_edges_adds_branch_table_trampolines() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(kind, value):\n    return value\n",
        )
        .expect("source should lower");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let function = typed
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");

        let jump_from = function.name_gen.next_block_name();
        let branch_from = function.name_gen.next_block_name();
        let target = function.name_gen.next_block_name();
        function.blocks = vec![
            Block::new_with_extra(
                jump_from,
                Vec::new(),
                BlockTerm::Jump(BlockEdge::new(target)),
                Vec::new(),
                None,
                TypedBlockExtra::default(),
            ),
            Block::new_with_extra(
                branch_from,
                Vec::new(),
                BlockTerm::BranchTable(block_py::TermBranchTable {
                    index: typed_load_temp(&local_name("kind", 0)),
                    targets: vec![target],
                    default_label: target,
                }),
                Vec::new(),
                None,
                TypedBlockExtra::default(),
            ),
            Block::new_with_extra(
                target,
                Vec::new(),
                BlockTerm::Return(typed_load_temp(&local_name("value", 1))),
                Vec::new(),
                None,
                TypedBlockExtra::default(),
            ),
        ];

        let object = TypedVirtualObjectId(7);
        let field = TypedVirtualFieldRef {
            object,
            field_name: "stop".to_string(),
        };
        let mut jump_state = TypedVirtualLoweringState::default();
        jump_state
            .virtual_state
            .fields
            .insert(field.clone(), local_name("jump_stop", 2));
        let mut branch_state = TypedVirtualLoweringState::default();
        branch_state
            .virtual_state
            .fields
            .insert(field, local_name("branch_stop", 3));
        let analysis = TypedVirtualLoweringAnalysis {
            block_in: HashMap::new(),
            body_before_instr: HashMap::new(),
            block_before_term: HashMap::new(),
            block_out: HashMap::from([(jump_from, jump_state), (branch_from, branch_state)]),
            edge_out: HashMap::new(),
        };

        assert_eq!(
            split_typed_virtual_field_block_param_edges(
                function,
                &analysis,
                &HashSet::from([object]),
            ),
            1,
            "branch-table joins needing field phis should be split through jump trampolines"
        );
        let BlockTerm::BranchTable(branch) = &function
            .blocks
            .iter()
            .find(|block| block.label == branch_from)
            .expect("branch block should remain present")
            .term
        else {
            panic!("branch source should stay a branch table");
        };
        let trampoline = branch.default_label;
        assert_ne!(trampoline, target);
        assert!(branch.targets.iter().all(|label| *label == trampoline));
        assert!(
            function.blocks.iter().any(|block| {
                block.label == trampoline
                    && matches!(
                        &block.term,
                        BlockTerm::Jump(edge) if edge.target == target && edge.args.is_empty()
                    )
            }),
            "the split edge should forward through a plain jump block"
        );
        let predecessors = typed_scalar_block_predecessor_edges(function);
        assert!(
            predecessors
                .get(&target)
                .expect("target should retain predecessors")
                .iter()
                .all(|edge| edge.explicit_args.is_some()),
            "target predecessors should become argument-capable jump edges"
        );
    }
}

fn rewrite_typed_field_scalar_children(
    expr: &mut InstrTyped,
    state: &mut TypedVirtualLoweringState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    match expr {
        InstrTyped::Truthy(op) => rewrite_typed_field_scalar_expr(
            op.value.as_mut(),
            state,
            module_constants,
            stats,
            rewrite,
        ),
        InstrTyped::BinOp(op) => {
            rewrite_typed_field_scalar_expr(
                op.left.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.right.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::Tuple(op) => {
            for value in &mut op.values {
                rewrite_typed_field_scalar_expr(value, state, module_constants, stats, rewrite);
            }
        }
        InstrTyped::UnaryOp(op) => rewrite_typed_field_scalar_expr(
            op.operand.as_mut(),
            state,
            module_constants,
            stats,
            rewrite,
        ),
        InstrTyped::CalleeFunctionId(op) => rewrite_typed_field_scalar_expr(
            op.value.as_mut(),
            state,
            module_constants,
            stats,
            rewrite,
        ),
        InstrTyped::CallTyped(op) => {
            rewrite_typed_field_scalar_expr(
                op.func.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_call_args(
                &mut op.args,
                &mut op.keywords,
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::GuardedCallableCallTyped(op) => {
            rewrite_typed_field_scalar_expr(
                op.func.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_call_args(
                &mut op.args,
                &mut op.keywords,
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::GuardedMethodCallTyped(op) => {
            rewrite_typed_field_scalar_expr(
                op.func.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_call_args(
                &mut op.args,
                &mut op.keywords,
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::DirectCallableCallTyped(op) => {
            rewrite_typed_field_scalar_expr(
                op.func.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_positional_args(
                &mut op.args,
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::DirectMethodCallTyped(op) => {
            rewrite_typed_field_scalar_expr(
                op.receiver.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_positional_args(
                &mut op.args,
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::CallDirect(op) => {
            rewrite_typed_field_scalar_expr(
                op.callable.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_call_args(
                &mut op.args,
                &mut op.keywords,
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::GetItem(op) => {
            rewrite_typed_field_scalar_expr(
                op.value.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.index.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::SetItem(op) => {
            rewrite_typed_field_scalar_expr(
                op.value.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.index.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.replacement.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::DelItem(op) => {
            rewrite_typed_field_scalar_expr(
                op.value.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.index.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::Store(op) => rewrite_typed_field_scalar_expr(
            op.value.as_mut(),
            state,
            module_constants,
            stats,
            rewrite,
        ),
        InstrTyped::MakeFunctionWithClosure(op) => {
            rewrite_typed_field_scalar_expr(
                op.captures.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.param_defaults.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
            rewrite_typed_field_scalar_expr(
                op.annotate_fn.as_mut(),
                state,
                module_constants,
                stats,
                rewrite,
            );
        }
        InstrTyped::Load(_)
        | InstrTyped::GetAttrTyped(_)
        | InstrTyped::SetAttrTyped(_)
        | InstrTyped::DirectCallGuardTest(_)
        | InstrTyped::Del(_)
        | InstrTyped::MakeCell(_)
        | InstrTyped::IncrementCounter(_)
        | InstrTyped::CellRef(_) => {}
    }
}

fn rewrite_typed_field_scalar_call_args(
    args: &mut [CallArgPositional<InstrTyped>],
    keywords: &mut [CallArgKeyword<InstrTyped>],
    state: &mut TypedVirtualLoweringState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    rewrite_typed_field_scalar_positional_args(args, state, module_constants, stats, rewrite);
    for keyword in keywords {
        rewrite_typed_field_scalar_expr(
            keyword.expr_mut(),
            state,
            module_constants,
            stats,
            rewrite,
        );
    }
}

fn rewrite_typed_field_scalar_positional_args(
    args: &mut [CallArgPositional<InstrTyped>],
    state: &mut TypedVirtualLoweringState,
    module_constants: &[ConstantExpr],
    stats: &mut TypedFieldScalarizationStats,
    rewrite: bool,
) {
    for arg in args {
        rewrite_typed_field_scalar_expr(arg.expr_mut(), state, module_constants, stats, rewrite);
    }
}

fn typed_virtual_objects_in_expr(
    expr: &InstrTyped,
    state: &TypedVirtualLoweringState,
) -> HashSet<TypedVirtualObjectId> {
    struct Collector<'a> {
        state: &'a TypedVirtualLoweringState,
        objects: HashSet<TypedVirtualObjectId>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some(location) = typed_instr_local_load_location(expr)
                && let Some(object) = self.state.object_for_location(location)
            {
                self.objects.insert(object);
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        state,
        objects: HashSet::new(),
    };
    collector.visit_instr(expr);
    collector.objects
}

pub(super) fn typed_constant_string<'a>(
    expr: &InstrTyped,
    module_constants: &'a [ConstantExpr],
) -> Option<&'a str> {
    let InstrTyped::Load(load) = expr else {
        return None;
    };
    let constant_index = load.name.location.as_constant()? as usize;
    match module_constants.get(constant_index)? {
        ConstantExpr::Literal(value) => match value.as_literal() {
            Literal::StringLiteral(value) => Some(value.value.as_str()),
            Literal::BytesLiteral(_) | Literal::NumberLiteral(_) => None,
        },
        ConstantExpr::RuntimeName(_) => None,
    }
}
