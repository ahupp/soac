use super::*;
use soac_core::block_py::FunctionKind;
use soac_ir_typed::{ProvenanceFact, TypedGeneratorInstancePlan, TypedGeneratorResumePlan};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrustedOwnerState {
    pub locals: HashMap<LocalLocation, TypedAttrOwnerRef>,
    pub preserved_owners: HashMap<PreservedLocation, TypedAttrOwnerRef>,
    pub runtime_names: HashMap<LocalLocation, RuntimeName>,
    pub preserved_runtime_names: HashMap<PreservedLocation, RuntimeName>,
    pub i64_values: HashMap<LocalLocation, i64>,
    pub preserved_i64_values: HashMap<PreservedLocation, i64>,
    pub object_origins: HashMap<LocalLocation, InstrId>,
    pub preserved_object_origins: HashMap<PreservedLocation, InstrId>,
    pub local_functions: HashMap<LocalLocation, RuntimeFunctionId>,
    pub preserved_functions: HashMap<PreservedLocation, RuntimeFunctionId>,
    pub function_fields: HashMap<(InstrId, String), RuntimeFunctionId>,
    pub resume_functions: HashMap<LocalLocation, TrustedResumeFunctionFact>,
    pub preserved_resume_functions: HashMap<PreservedLocation, TrustedResumeFunctionFact>,
    pub escaped_origins: HashSet<InstrId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedResumeFunctionFact {
    pub function_id: RuntimeFunctionId,
    pub origins: HashSet<InstrId>,
}

impl TrustedResumeFunctionFact {
    fn new(function_id: RuntimeFunctionId, origin: InstrId) -> Self {
        Self {
            function_id,
            origins: HashSet::from([origin]),
        }
    }

    fn exact_origin(&self) -> Option<InstrId> {
        (self.origins.len() == 1)
            .then(|| self.origins.iter().copied().next())
            .flatten()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrustedOwnerStateAnalysis {
    pub body_before_instr: HashMap<TypedVirtualBodyInstr, TrustedOwnerState>,
    pub block_before_term: HashMap<BlockLabel, TrustedOwnerState>,
}

#[derive(Clone, Debug)]
pub struct TrustedOwnerPredecessorEdge {
    pub from: BlockLabel,
    pub explicit_args: Option<Vec<BlockArg>>,
    pub explicit_local_remaps: Option<Vec<(LocalLocation, LocalLocation)>>,
    pub branch_gate: Option<TrustedOwnerBranchGate>,
}

#[derive(Clone, Debug)]
pub enum TrustedOwnerBranchGate {
    Case {
        index_name: ResolvedName,
        case_index: usize,
    },
    Default {
        index_name: ResolvedName,
        case_count: usize,
    },
}

type TrustedOwnerResumeCaseKey = (LocalLocation, i64);

pub fn trusted_owner_state_for_name<'a>(
    name: &ResolvedName,
    state: &'a TrustedOwnerState,
) -> Option<&'a TypedAttrOwnerRef> {
    if let Some(location) = name.local_location() {
        return state.locals.get(&location);
    }
    state.preserved_owners.get(&name.preserved_location()?)
}

fn trusted_runtime_name_for_name(
    name: &ResolvedName,
    state: &TrustedOwnerState,
) -> Option<RuntimeName> {
    if let Some(location) = name.local_location() {
        return state.runtime_names.get(&location).copied();
    }
    state
        .preserved_runtime_names
        .get(&name.preserved_location()?)
        .copied()
}

fn trusted_module_constant_i64_value(module_constants: &[ConstantExpr], index: u32) -> Option<i64> {
    let ConstantExpr::Literal(value) = module_constants.get(index as usize)? else {
        return None;
    };
    let Literal::NumberLiteral(number) = value.as_literal() else {
        return None;
    };
    let NumberLiteralValue::Int(value) = &number.value else {
        return None;
    };
    value.as_i64()
}

fn trusted_i64_value_for_name(
    name: &ResolvedName,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> Option<i64> {
    if let Some(location) = name.local_location() {
        return state.i64_values.get(&location).copied();
    }
    if let Some(location) = name.preserved_location() {
        return state.preserved_i64_values.get(&location).copied();
    }
    name.location
        .as_constant()
        .and_then(|index| trusted_module_constant_i64_value(module_constants, index))
}

fn trusted_i64_value_for_expr(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> Option<i64> {
    let InstrTyped::Load(load) = expr else {
        return None;
    };
    trusted_i64_value_for_name(&load.name, state, module_constants)
}

pub fn trusted_object_origin_for_name(
    name: &ResolvedName,
    state: &TrustedOwnerState,
) -> Option<InstrId> {
    if let Some(location) = name.local_location() {
        return state.object_origins.get(&location).copied();
    }
    state
        .preserved_object_origins
        .get(&name.preserved_location()?)
        .copied()
}

fn typed_expr_runtime_name_provenance(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
) -> Option<RuntimeName> {
    if let InstrTyped::Load(load) = expr
        && let Some(runtime_name) = load.name.runtime_name_id()
    {
        return Some(runtime_name);
    }
    let facts = expr.result_facts()?.as_pyobj()?;
    let ProvenanceFact::ModuleConstant(index) = facts.provenance else {
        return None;
    };
    match module_constants.get(index as usize) {
        Some(ConstantExpr::RuntimeName(runtime_name)) => Some(*runtime_name),
        _ => None,
    }
}

pub fn trusted_runtime_name_for_expr(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> Option<RuntimeName> {
    if let InstrTyped::Load(load) = expr {
        if let Some(runtime_name) = load.name.runtime_name_id() {
            return Some(runtime_name);
        }
        if let Some(runtime_name) = trusted_runtime_name_for_name(&load.name, state) {
            return Some(runtime_name);
        }
        if let Some(index) = load.name.location.as_constant()
            && let Some(ConstantExpr::RuntimeName(runtime_name)) =
                module_constants.get(index as usize)
        {
            return Some(*runtime_name);
        }
    }
    typed_expr_runtime_name_provenance(expr, module_constants)
}

pub fn trusted_function_id_for_expr(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
) -> Option<RuntimeFunctionId> {
    match expr {
        InstrTyped::MakeFunctionWithClosure(op) => Some(op.function_id),
        InstrTyped::Load(load) => {
            if let Some(location) = load.name.local_location() {
                return state.local_functions.get(&location).copied();
            }
            state
                .preserved_functions
                .get(&load.name.preserved_location()?)
                .copied()
        }
        _ => None,
    }
}

fn trusted_field_function_id_for_expr(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> Option<(InstrId, RuntimeFunctionId)> {
    let InstrTyped::GetAttrTyped(get_attr) = expr else {
        return None;
    };
    let field_name = typed_constant_string(get_attr.attr.as_ref(), module_constants)?;
    let InstrTyped::Load(receiver) = get_attr.value.as_ref() else {
        return None;
    };
    let origin = trusted_object_origin_for_name(&receiver.name, state)?;
    let function_id = state
        .function_fields
        .get(&(origin, field_name.to_string()))
        .copied()?;
    Some((origin, function_id))
}

fn trusted_resume_function_fact_for_name<'a>(
    name: &ResolvedName,
    state: &'a TrustedOwnerState,
) -> Option<&'a TrustedResumeFunctionFact> {
    if let Some(location) = name.local_location() {
        return state.resume_functions.get(&location);
    }
    state
        .preserved_resume_functions
        .get(&name.preserved_location()?)
}

fn trusted_generator_resume_call_target(
    resume_function: &InstrTyped,
    owner: &Load<InstrTyped>,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> Option<(Option<InstrId>, RuntimeFunctionId, Vec<InstrId>)> {
    if let Some((origin, function_id)) =
        trusted_field_function_id_for_expr(resume_function, state, module_constants)
    {
        return Some((Some(origin), function_id, vec![origin]));
    }
    trusted_resume_function_fact_for_name(&owner.name, state)
        .map(|fact| {
            let mut candidate_origins = fact.origins.iter().copied().collect::<Vec<_>>();
            candidate_origins.sort_by_key(|origin| origin.index());
            (fact.exact_origin(), fact.function_id, candidate_origins)
        })
}

pub fn trusted_fully_virtual_constructor_owner(owner: &TypedAttrOwnerRef) -> bool {
    matches!(
        owner,
        TypedAttrOwnerRef::TypeKey {
            module_name,
            qualname,
        } if module_name == "soac.runtime"
            && matches!(
                qualname.as_str(),
                "range" | "IterRange" | "ClosureGenerator" | "ClosureAsyncGenerator"
            )
    )
}

pub fn trusted_generator_instance_owner(
    plan: &TypedGeneratorInstancePlan,
) -> Option<TypedAttrOwnerRef> {
    let qualname = match plan.kind {
        FunctionKind::Generator => "ClosureGenerator",
        FunctionKind::Coroutine => "Coroutine",
        FunctionKind::AsyncGenerator => "ClosureAsyncGenerator",
        FunctionKind::Function => return None,
    };
    Some(TypedAttrOwnerRef::TypeKey {
        module_name: "soac.runtime".to_string(),
        qualname: qualname.to_string(),
    })
}

fn trusted_identity_iter_owner_type(owner_type_ref: &TypedAttrOwnerRef) -> bool {
    matches!(
        owner_type_ref,
        TypedAttrOwnerRef::TypeKey {
            module_name,
            qualname,
        } if module_name == "soac.runtime"
            && matches!(qualname.as_str(), "ClosureGenerator" | "IterRange")
    )
}

fn trusted_identity_iter_store_value(
    value: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> Option<(TypedAttrOwnerRef, InstrId)> {
    let InstrTyped::CallTyped(call) = value else {
        return None;
    };
    let is_iter_call = match &call.access {
        soac_ir_typed::TypedCallAccessPlan::Generic => {
            typed_expr_is_runtime_name_load(call.func.as_ref(), RuntimeName::Iter, module_constants)
        }
        soac_ir_typed::TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
            runtime_name, ..
        } => *runtime_name == RuntimeName::Iter,
        _ => false,
    };
    if !is_iter_call || !call.keywords.is_empty() {
        return None;
    }
    let Some(CallArgPositional::Positional(InstrTyped::Load(receiver))) = call.args.first() else {
        return None;
    };
    let owner_type_ref = trusted_owner_state_for_name(&receiver.name, state)?.clone();
    if !trusted_identity_iter_owner_type(&owner_type_ref) {
        return None;
    }
    let origin = trusted_object_origin_for_name(&receiver.name, state)?;
    Some((owner_type_ref, origin))
}

fn trusted_materialized_constructor_owner(
    value: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
) -> Option<TypedAttrOwnerRef> {
    let (func, args, init_plan) = match value {
        InstrTyped::CallTyped(call) => (
            call.func.as_ref(),
            call.args.as_slice(),
            call.extra.constructor_init_plan(),
        ),
        InstrTyped::DirectCallableCallTyped(call) => (
            call.func.as_ref(),
            call.args.as_slice(),
            call.extra.constructor_init_plan(),
        ),
        _ => return None,
    };
    if let Some(init_plan) = init_plan
        && let Some(owner_type_ref) =
            trusted_constructor_init_owners.get(&init_plan.init_function_id)
    {
        return Some(owner_type_ref.clone());
    }
    if init_plan.is_none()
        && !typed_expr_is_runtime_name_load(func, RuntimeName::ConstructorCall, module_constants)
    {
        return None;
    }
    let CallArgPositional::Positional(class_expr) = args.first()? else {
        return None;
    };
    let runtime_name = trusted_runtime_name_for_expr(class_expr, state, module_constants)?;
    let owner_type_ref = TypedAttrOwnerRef::TypeKey {
        module_name: "soac.runtime".to_string(),
        qualname: runtime_name.name().to_string(),
    };
    trusted_fully_virtual_constructor_owner(&owner_type_ref).then_some(owner_type_ref)
}

fn trusted_owner_state_for_store_value(
    value: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
) -> Option<TypedAttrOwnerRef> {
    if let Some((owner_type_ref, _)) =
        trusted_identity_iter_store_value(value, state, module_constants)
    {
        return Some(owner_type_ref);
    }
    if let Some(plan) = value.generator_instance_plan()
        && let Some(owner_type_ref) = trusted_generator_instance_owner(plan)
    {
        return Some(owner_type_ref);
    }
    if let Some(instr_id) = value.try_semantic_instr_id()
        && let Some(owner_type_ref) = trusted_constructor_calls.get(&instr_id)
    {
        return Some(owner_type_ref.clone());
    }
    if let Some(owner_type_ref) = trusted_materialized_constructor_owner(
        value,
        state,
        module_constants,
        trusted_constructor_init_owners,
    ) {
        return Some(owner_type_ref);
    }
    let InstrTyped::Load(load) = value else {
        return None;
    };
    trusted_owner_state_for_name(&load.name, state).cloned()
}

fn trusted_object_origin_for_store_value(
    value: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
) -> Option<InstrId> {
    if let Some((_, origin)) = trusted_identity_iter_store_value(value, state, module_constants) {
        return Some(origin);
    }
    if value.generator_instance_plan().is_some() {
        return value.try_semantic_instr_id();
    }
    if let Some(instr_id) = value.try_semantic_instr_id()
        && trusted_constructor_calls.contains_key(&instr_id)
    {
        return Some(instr_id);
    }
    let InstrTyped::Load(load) = value else {
        return None;
    };
    trusted_object_origin_for_name(&load.name, state)
}

fn trusted_runtime_name_for_store_value(
    value: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> Option<RuntimeName> {
    trusted_runtime_name_for_expr(value, state, module_constants)
}

fn trusted_resume_function_for_store_value(
    value: &InstrTyped,
    state: &TrustedOwnerState,
) -> Option<TrustedResumeFunctionFact> {
    if let Some(plan) = value.generator_instance_plan() {
        return Some(TrustedResumeFunctionFact::new(
            plan.function_id,
            value.try_semantic_instr_id()?,
        ));
    }
    let InstrTyped::Load(load) = value else {
        return None;
    };
    trusted_resume_function_fact_for_name(&load.name, state).cloned()
}

fn trusted_generator_resume_call_plan_from_parts(
    instr_id: Option<InstrId>,
    func: &InstrTyped,
    args: &[CallArgPositional<InstrTyped>],
    has_keywords: bool,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> Option<TypedGeneratorResumePlan> {
    let runtime_name = trusted_runtime_name_for_expr(func, state, module_constants);
    if runtime_name != Some(RuntimeName::ResumeGenerator) {
        if let InstrTyped::Load(load) = func
            && load.name.id_str() == RuntimeName::ResumeGenerator.name()
        {
            tracing::debug!(
                target: "soac_generator_resume_planning",
                instr_id = ?instr_id,
                location = ?load.name.location,
                runtime_name = ?runtime_name,
                "typed_generator_resume_plan_skipped_untrusted_resume_name",
            );
        }
        return None;
    }
    if has_keywords {
        tracing::debug!(
            target: "soac_generator_resume_planning",
            instr_id = ?instr_id,
            "typed_generator_resume_plan_skipped_keywords",
        );
        return None;
    }
    let [
        CallArgPositional::Positional(resume_function),
        CallArgPositional::Positional(owner),
        ..,
    ] = args
    else {
        tracing::debug!(
            target: "soac_generator_resume_planning",
            instr_id = ?instr_id,
            arg_count = args.len(),
            "typed_generator_resume_plan_skipped_arg_shape",
        );
        return None;
    };
    let InstrTyped::Load(owner) = owner else {
        tracing::debug!(
            target: "soac_generator_resume_planning",
            instr_id = ?instr_id,
            owner = ?owner,
            "typed_generator_resume_plan_skipped_non_load_owner",
        );
        return None;
    };
    let Some((resume_origin, function_id, candidate_origins)) =
        trusted_generator_resume_call_target(resume_function, owner, state, module_constants)
    else {
        tracing::debug!(
            target: "soac_generator_resume_planning",
            instr_id = ?instr_id,
            resume_function = ?resume_function,
            "typed_generator_resume_plan_skipped_missing_resume_function",
        );
        return None;
    };
    let generator_origin = trusted_object_origin_for_name(&owner.name, state);
    if let Some(resume_origin) = resume_origin {
        let Some(generator_origin) = generator_origin else {
            tracing::debug!(
                target: "soac_generator_resume_planning",
                instr_id = ?instr_id,
                owner_name = owner.name.id_str(),
                resume_origin = ?resume_origin,
                function_id = ?function_id,
                "typed_generator_resume_plan_skipped_missing_owner_origin",
            );
            return None;
        };
        if resume_origin != generator_origin {
            tracing::debug!(
                target: "soac_generator_resume_planning",
                instr_id = ?instr_id,
                generator_origin = ?generator_origin,
                resume_origin = ?resume_origin,
                function_id = ?function_id,
                "typed_generator_resume_plan_skipped_origin_mismatch",
            );
            return None;
        }
    }
    tracing::debug!(
        target: "soac_generator_resume_planning",
        instr_id = ?instr_id,
        generator_origin = ?generator_origin,
        function_id = ?function_id,
        "typed_generator_resume_plan_selected",
    );
    Some(TypedGeneratorResumePlan {
        function_id,
        generator_origin,
        candidate_origins,
    })
}

pub fn trusted_generator_resume_plan_for_expr(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> Option<(InstrId, TypedGeneratorResumePlan)> {
    let instr_id = expr.try_semantic_instr_id()?;
    let plan = match expr {
        InstrTyped::CallTyped(call) => trusted_generator_resume_call_plan_from_parts(
            Some(instr_id),
            call.func.as_ref(),
            &call.args,
            !call.keywords.is_empty(),
            state,
            module_constants,
        ),
        InstrTyped::GuardedCallableCallTyped(call) => {
            trusted_generator_resume_call_plan_from_parts(
                Some(instr_id),
                call.func.as_ref(),
                &call.args,
                !call.keywords.is_empty(),
                state,
                module_constants,
            )
        }
        InstrTyped::DirectCallableCallTyped(call) => trusted_generator_resume_call_plan_from_parts(
            Some(instr_id),
            call.func.as_ref(),
            &call.args,
            false,
            state,
            module_constants,
        ),
        _ => return None,
    }?;
    Some((instr_id, plan))
}

fn trusted_escaping_object_origins_in_expr(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> HashSet<InstrId> {
    struct Collector<'a> {
        state: &'a TrustedOwnerState,
        module_constants: &'a [ConstantExpr],
        origins: HashSet<InstrId>,
    }

    impl Collector<'_> {
        fn visit_call_arg(&mut self, arg: &CallArgPositional<InstrTyped>) {
            match arg {
                CallArgPositional::Positional(expr) | CallArgPositional::Starred(expr) => {
                    self.visit_instr(expr);
                }
            }
        }
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if trusted_generator_resume_plan_for_expr(expr, self.state, self.module_constants)
                .is_some()
            {
                match expr {
                    InstrTyped::CallTyped(call) => {
                        for arg in call.args.iter().skip(3) {
                            self.visit_call_arg(arg);
                        }
                        for keyword in &call.keywords {
                            match keyword {
                                CallArgKeyword::Named { value, .. }
                                | CallArgKeyword::Starred(value) => {
                                    self.visit_instr(value);
                                }
                            }
                        }
                    }
                    InstrTyped::GuardedCallableCallTyped(call) => {
                        for arg in call.args.iter().skip(3) {
                            self.visit_call_arg(arg);
                        }
                        for keyword in &call.keywords {
                            match keyword {
                                CallArgKeyword::Named { value, .. }
                                | CallArgKeyword::Starred(value) => {
                                    self.visit_instr(value);
                                }
                            }
                        }
                    }
                    InstrTyped::DirectCallableCallTyped(call) => {
                        for arg in call.args.iter().skip(3) {
                            self.visit_call_arg(arg);
                        }
                    }
                    _ => unreachable!("trusted resume plan must come from a call expression"),
                }
                return;
            }
            if let InstrTyped::Load(load) = expr
                && let Some(origin) = trusted_object_origin_for_name(&load.name, self.state)
            {
                self.origins.insert(origin);
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        state,
        module_constants,
        origins: HashSet::new(),
    };
    collector.visit_instr(expr);
    collector.origins
}

fn mark_trusted_owner_escapes_for_instr(
    instr: &InstrTyped,
    state: &mut TrustedOwnerState,
    module_constants: &[ConstantExpr],
) {
    let escaped = match instr {
        InstrTyped::Store(store)
            if store.name.local_location().is_some()
                && matches!(store.value.as_ref(), InstrTyped::Load(_)) =>
        {
            HashSet::new()
        }
        InstrTyped::Store(store) => {
            trusted_escaping_object_origins_in_expr(store.value.as_ref(), state, module_constants)
        }
        InstrTyped::SetAttrTyped(op) => trusted_escaping_object_origins_in_expr(
            op.replacement.as_ref(),
            state,
            module_constants,
        ),
        _ => trusted_escaping_object_origins_in_expr(instr, state, module_constants),
    };
    state.escaped_origins.extend(escaped);
    state.resume_functions.retain(|_, fact| {
        fact.origins.is_disjoint(&state.escaped_origins)
    });
    state.preserved_resume_functions.retain(|_, fact| {
        fact.origins.is_disjoint(&state.escaped_origins)
    });
}

fn transfer_trusted_owner_instr(
    instr: &InstrTyped,
    state: &mut TrustedOwnerState,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
) {
    mark_trusted_owner_escapes_for_instr(instr, state, module_constants);
    match instr {
        InstrTyped::Store(store) => {
            if let Some(location) = store.name.local_location() {
                if let Some(owner_type_ref) = trusted_owner_state_for_store_value(
                    store.value.as_ref(),
                    state,
                    module_constants,
                    trusted_constructor_calls,
                    trusted_constructor_init_owners,
                ) {
                    state.locals.insert(location, owner_type_ref);
                } else {
                    state.locals.remove(&location);
                }
                if let Some(runtime_name) = trusted_runtime_name_for_store_value(
                    store.value.as_ref(),
                    state,
                    module_constants,
                ) {
                    state.runtime_names.insert(location, runtime_name);
                } else {
                    state.runtime_names.remove(&location);
                }
                if let Some(value) =
                    trusted_i64_value_for_expr(store.value.as_ref(), state, module_constants)
                {
                    state.i64_values.insert(location, value);
                } else {
                    state.i64_values.remove(&location);
                }
                let resume_function =
                    trusted_resume_function_for_store_value(store.value.as_ref(), state);
                if let Some(origin) = trusted_object_origin_for_store_value(
                    store.value.as_ref(),
                    state,
                    module_constants,
                    trusted_constructor_calls,
                ) {
                    state.object_origins.insert(location, origin);
                    if store.value.try_semantic_instr_id() == Some(origin) {
                        state.escaped_origins.remove(&origin);
                    }
                } else {
                    state.object_origins.remove(&location);
                }
                if let Some(fact) = resume_function {
                    state.resume_functions.insert(location, fact);
                } else {
                    state.resume_functions.remove(&location);
                }
                if let Some(function_id) = trusted_function_id_for_expr(store.value.as_ref(), state)
                {
                    state.local_functions.insert(location, function_id);
                } else {
                    state.local_functions.remove(&location);
                }
                if let Some(origin) = store.value.try_semantic_instr_id()
                    && let Some(plan) = store.value.generator_instance_plan()
                {
                    state
                        .function_fields
                        .insert((origin, "_resume_function".to_string()), plan.function_id);
                }
            }
            if let Some(location) = store.name.preserved_location() {
                if let Some(owner_type_ref) = trusted_owner_state_for_store_value(
                    store.value.as_ref(),
                    state,
                    module_constants,
                    trusted_constructor_calls,
                    trusted_constructor_init_owners,
                ) {
                    state.preserved_owners.insert(location, owner_type_ref);
                } else {
                    state.preserved_owners.remove(&location);
                }
                if let Some(runtime_name) = trusted_runtime_name_for_store_value(
                    store.value.as_ref(),
                    state,
                    module_constants,
                ) {
                    state.preserved_runtime_names.insert(location, runtime_name);
                } else {
                    state.preserved_runtime_names.remove(&location);
                }
                if let Some(value) =
                    trusted_i64_value_for_expr(store.value.as_ref(), state, module_constants)
                {
                    state.preserved_i64_values.insert(location, value);
                } else {
                    state.preserved_i64_values.remove(&location);
                }
                let resume_function =
                    trusted_resume_function_for_store_value(store.value.as_ref(), state);
                if let Some(origin) = trusted_object_origin_for_store_value(
                    store.value.as_ref(),
                    state,
                    module_constants,
                    trusted_constructor_calls,
                ) {
                    state.preserved_object_origins.insert(location, origin);
                    if store.value.try_semantic_instr_id() == Some(origin) {
                        state.escaped_origins.remove(&origin);
                    }
                } else {
                    state.preserved_object_origins.remove(&location);
                }
                if let Some(fact) = resume_function {
                    state
                        .preserved_resume_functions
                        .insert(location, fact);
                } else {
                    state.preserved_resume_functions.remove(&location);
                }
                if let Some(function_id) = trusted_function_id_for_expr(store.value.as_ref(), state)
                {
                    state.preserved_functions.insert(location, function_id);
                } else {
                    state.preserved_functions.remove(&location);
                }
            }
        }
        InstrTyped::Del(del) => {
            if let Some(location) = del.name.local_location() {
                state.locals.remove(&location);
                state.runtime_names.remove(&location);
                state.i64_values.remove(&location);
                state.object_origins.remove(&location);
                state.local_functions.remove(&location);
                state.resume_functions.remove(&location);
            }
            if let Some(location) = del.name.preserved_location() {
                state.preserved_owners.remove(&location);
                state.preserved_runtime_names.remove(&location);
                state.preserved_i64_values.remove(&location);
                state.preserved_object_origins.remove(&location);
                state.preserved_functions.remove(&location);
                state.preserved_resume_functions.remove(&location);
            }
        }
        InstrTyped::SetAttrTyped(op) => {
            let Some(field_name) = typed_constant_string(op.attr.as_ref(), module_constants) else {
                return;
            };
            let InstrTyped::Load(receiver) = op.value.as_ref() else {
                return;
            };
            let Some(origin) = trusted_object_origin_for_name(&receiver.name, state) else {
                return;
            };
            let key = (origin, field_name.to_string());
            if let Some(function_id) = trusted_function_id_for_expr(op.replacement.as_ref(), state)
            {
                state.function_fields.insert(key, function_id);
                if field_name == "_resume_function" {
                    if let Some(location) = receiver.name.local_location() {
                        state.resume_functions.insert(
                            location,
                            TrustedResumeFunctionFact::new(function_id, origin),
                        );
                    }
                    if let Some(location) = receiver.name.preserved_location() {
                        state.preserved_resume_functions.insert(
                            location,
                            TrustedResumeFunctionFact::new(function_id, origin),
                        );
                    }
                }
            } else {
                state.function_fields.remove(&key);
                if field_name == "_resume_function" {
                    if let Some(location) = receiver.name.local_location() {
                        state.resume_functions.remove(&location);
                    }
                    if let Some(location) = receiver.name.preserved_location() {
                        state.preserved_resume_functions.remove(&location);
                    }
                }
            }
        }
        _ => {}
    }
}

pub fn merge_trusted_owner_states(states: &[TrustedOwnerState]) -> TrustedOwnerState {
    let Some(first) = states.first() else {
        return TrustedOwnerState::default();
    };
    let locals = first
        .locals
        .iter()
        .filter(|(location, owner)| {
            states
                .iter()
                .all(|state| state.locals.get(location) == Some(*owner))
        })
        .map(|(location, owner)| (*location, owner.clone()))
        .collect();
    let preserved_owners = first
        .preserved_owners
        .iter()
        .filter(|(location, owner)| {
            states
                .iter()
                .all(|state| state.preserved_owners.get(location) == Some(*owner))
        })
        .map(|(location, owner)| (*location, owner.clone()))
        .collect();
    let runtime_names = first
        .runtime_names
        .iter()
        .filter(|(location, runtime_name)| {
            states
                .iter()
                .all(|state| state.runtime_names.get(location) == Some(*runtime_name))
        })
        .map(|(location, runtime_name)| (*location, *runtime_name))
        .collect();
    let i64_values = first
        .i64_values
        .iter()
        .filter(|(location, value)| {
            states
                .iter()
                .all(|state| state.i64_values.get(location) == Some(*value))
        })
        .map(|(location, value)| (*location, *value))
        .collect();
    let preserved_runtime_names = first
        .preserved_runtime_names
        .iter()
        .filter(|(location, runtime_name)| {
            states
                .iter()
                .all(|state| state.preserved_runtime_names.get(location) == Some(*runtime_name))
        })
        .map(|(location, runtime_name)| (*location, *runtime_name))
        .collect();
    let preserved_i64_values = first
        .preserved_i64_values
        .iter()
        .filter(|(location, value)| {
            states
                .iter()
                .all(|state| state.preserved_i64_values.get(location) == Some(*value))
        })
        .map(|(location, value)| (*location, *value))
        .collect();
    let object_origins = first
        .object_origins
        .iter()
        .filter(|(location, origin)| {
            states
                .iter()
                .all(|state| state.object_origins.get(location) == Some(*origin))
        })
        .map(|(location, origin)| (*location, *origin))
        .collect();
    let preserved_object_origins = first
        .preserved_object_origins
        .iter()
        .filter(|(location, origin)| {
            states
                .iter()
                .all(|state| state.preserved_object_origins.get(location) == Some(*origin))
        })
        .map(|(location, origin)| (*location, *origin))
        .collect();
    let local_functions = first
        .local_functions
        .iter()
        .filter(|(location, function_id)| {
            states
                .iter()
                .all(|state| state.local_functions.get(location) == Some(*function_id))
        })
        .map(|(location, function_id)| (*location, *function_id))
        .collect();
    let resume_functions = first
        .resume_functions
        .iter()
        .filter_map(|(location, fact)| {
            let mut origins = fact.origins.clone();
            for state in states.iter().skip(1) {
                let other = state.resume_functions.get(location)?;
                if other.function_id != fact.function_id {
                    return None;
                }
                origins.extend(other.origins.iter().copied());
            }
            Some((
                *location,
                TrustedResumeFunctionFact {
                    function_id: fact.function_id,
                    origins,
                },
            ))
        })
        .collect();
    let preserved_functions = first
        .preserved_functions
        .iter()
        .filter(|(location, function_id)| {
            states
                .iter()
                .all(|state| state.preserved_functions.get(location) == Some(*function_id))
        })
        .map(|(location, function_id)| (*location, *function_id))
        .collect();
    let preserved_resume_functions = first
        .preserved_resume_functions
        .iter()
        .filter_map(|(location, fact)| {
            let mut origins = fact.origins.clone();
            for state in states.iter().skip(1) {
                let other = state.preserved_resume_functions.get(location)?;
                if other.function_id != fact.function_id {
                    return None;
                }
                origins.extend(other.origins.iter().copied());
            }
            Some((
                *location,
                TrustedResumeFunctionFact {
                    function_id: fact.function_id,
                    origins,
                },
            ))
        })
        .collect();
    let mut merged_function_fields = HashMap::new();
    for state in states {
        for (field, function_id) in &state.function_fields {
            match merged_function_fields.entry(field.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(Some(*function_id));
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    if entry.get().is_some_and(|current| current != *function_id) {
                        entry.insert(None);
                    }
                }
            }
        }
    }
    let function_fields = merged_function_fields
        .into_iter()
        .filter_map(|(field, function_id)| function_id.map(|function_id| (field, function_id)))
        .collect();
    let escaped_origins = states
        .iter()
        .flat_map(|state| state.escaped_origins.iter().copied())
        .collect();
    TrustedOwnerState {
        locals,
        preserved_owners,
        runtime_names,
        preserved_runtime_names,
        i64_values,
        preserved_i64_values,
        object_origins,
        preserved_object_origins,
        local_functions,
        preserved_functions,
        function_fields,
        resume_functions,
        preserved_resume_functions,
        escaped_origins,
    }
}

pub fn trusted_owner_block_predecessor_edges(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<BlockLabel, Vec<TrustedOwnerPredecessorEdge>> {
    let mut predecessors = HashMap::<BlockLabel, Vec<TrustedOwnerPredecessorEdge>>::new();
    for block in &function.blocks {
        match &block.term {
            BlockTerm::Jump(edge) => {
                predecessors
                    .entry(edge.target)
                    .or_default()
                    .push(TrustedOwnerPredecessorEdge {
                        from: block.label,
                        explicit_args: Some(edge.args.clone()),
                        explicit_local_remaps: None,
                        branch_gate: None,
                    });
            }
            BlockTerm::IfTerm(if_term) => {
                for target in [if_term.then_label, if_term.else_label] {
                    predecessors
                        .entry(target)
                        .or_default()
                        .push(TrustedOwnerPredecessorEdge {
                            from: block.label,
                            explicit_args: None,
                            explicit_local_remaps: None,
                            branch_gate: None,
                        });
                }
            }
            BlockTerm::BranchTable(branch) => {
                let index_name = match &branch.index {
                    InstrTyped::Load(load) => Some(load.name.clone()),
                    _ => None,
                };
                for (case_index, target) in branch.targets.iter().copied().enumerate() {
                    predecessors
                        .entry(target)
                        .or_default()
                        .push(TrustedOwnerPredecessorEdge {
                            from: block.label,
                            explicit_args: None,
                            explicit_local_remaps: None,
                            branch_gate: index_name.clone().map(|index_name| {
                                TrustedOwnerBranchGate::Case {
                                    index_name,
                                    case_index,
                                }
                            }),
                        });
                }
                predecessors.entry(branch.default_label).or_default().push(
                    TrustedOwnerPredecessorEdge {
                        from: block.label,
                        explicit_args: None,
                        explicit_local_remaps: None,
                        branch_gate: index_name.map(|index_name| TrustedOwnerBranchGate::Default {
                            index_name,
                            case_count: branch.targets.len(),
                        }),
                    },
                );
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
        }
        if let Some(exc_edge) = &block.exc_edge {
            predecessors
                .entry(exc_edge.target)
                .or_default()
                .push(TrustedOwnerPredecessorEdge {
                    from: block.label,
                    explicit_args: Some(exc_edge.args.clone()),
                    explicit_local_remaps: None,
                    branch_gate: None,
                });
        }
    }
    predecessors
}

fn typed_local_locations_by_name(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<String, LocalLocation> {
    function
        .storage_layout
        .as_ref()
        .map(|layout| {
            layout
                .stack_slots()
                .iter()
                .enumerate()
                .map(|(slot, name)| {
                    (
                        name.clone(),
                        LocalLocation(
                            u32::try_from(slot)
                                .expect("stack slot index should fit in LocalLocation"),
                        ),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn trusted_owner_local_remaps_for_edge(
    target: &soac_ir_typed::TypedBlock,
    explicit_args: &[BlockArg],
    local_locations_by_name: &HashMap<String, LocalLocation>,
) -> Vec<(LocalLocation, LocalLocation)> {
    target
        .params
        .iter()
        .zip(explicit_args)
        .filter_map(|(param, arg)| {
            if param.role != BlockParamRole::Value {
                return None;
            }
            let BlockArg::Name(source_name) = arg else {
                return None;
            };
            Some((
                local_locations_by_name.get(source_name).copied()?,
                local_locations_by_name.get(&param.name).copied()?,
            ))
        })
        .collect()
}

pub fn remap_trusted_owner_state_for_edge(
    explicit_local_remaps: Option<&[(LocalLocation, LocalLocation)]>,
    state: &TrustedOwnerState,
) -> TrustedOwnerState {
    let Some(local_remaps) = explicit_local_remaps else {
        return state.clone();
    };
    let mut remapped = state.clone();
    for (source, target) in local_remaps {
        match remapped.locals.get(source).cloned() {
            Some(owner_type_ref) => {
                remapped.locals.insert(*target, owner_type_ref);
            }
            None => {
                remapped.locals.remove(target);
            }
        }
        match remapped.runtime_names.get(source).copied() {
            Some(runtime_name) => {
                remapped.runtime_names.insert(*target, runtime_name);
            }
            None => {
                remapped.runtime_names.remove(target);
            }
        }
        match remapped.i64_values.get(source).copied() {
            Some(value) => {
                remapped.i64_values.insert(*target, value);
            }
            None => {
                remapped.i64_values.remove(target);
            }
        }
        match remapped.object_origins.get(source).copied() {
            Some(origin) => {
                remapped.object_origins.insert(*target, origin);
            }
            None => {
                remapped.object_origins.remove(target);
            }
        }
        match remapped.resume_functions.get(source).cloned() {
            Some(fact) => {
                remapped.resume_functions.insert(*target, fact);
            }
            None => {
                remapped.resume_functions.remove(target);
            }
        }
        match remapped.local_functions.get(source).copied() {
            Some(function_id) => {
                remapped.local_functions.insert(*target, function_id);
            }
            None => {
                remapped.local_functions.remove(target);
            }
        }
    }
    remapped
}

fn trusted_owner_branch_gate_allows_state(
    gate: Option<&TrustedOwnerBranchGate>,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> bool {
    let Some(gate) = gate else {
        return true;
    };
    let index_name = match gate {
        TrustedOwnerBranchGate::Case { index_name, .. }
        | TrustedOwnerBranchGate::Default { index_name, .. } => index_name,
    };
    let Some(index) = trusted_i64_value_for_name(index_name, state, module_constants) else {
        return true;
    };
    match gate {
        TrustedOwnerBranchGate::Case { case_index, .. } => {
            usize::try_from(index).ok() == Some(*case_index)
        }
        TrustedOwnerBranchGate::Default { case_count, .. } => usize::try_from(index)
            .ok()
            .is_none_or(|index| index >= *case_count),
    }
}

fn trusted_owner_resume_case_key(
    gate: &TrustedOwnerBranchGate,
) -> Option<TrustedOwnerResumeCaseKey> {
    let TrustedOwnerBranchGate::Case {
        index_name,
        case_index,
    } = gate
    else {
        return None;
    };
    Some((
        index_name.local_location()?,
        i64::try_from(*case_index).ok()?,
    ))
}

fn merge_trusted_owner_resume_case_state(
    states: &mut HashMap<TrustedOwnerResumeCaseKey, TrustedOwnerState>,
    key: TrustedOwnerResumeCaseKey,
    next: TrustedOwnerState,
) -> bool {
    let Some(existing) = states.get(&key).cloned() else {
        states.insert(key, next);
        return true;
    };
    let merged = merge_trusted_owner_states(&[existing.clone(), next]);
    if merged == existing {
        return false;
    }
    states.insert(key, merged);
    true
}

fn trusted_owner_states_for_edge(
    edge: &TrustedOwnerPredecessorEdge,
    predecessors: &HashMap<BlockLabel, Vec<TrustedOwnerPredecessorEdge>>,
    labels: &HashMap<BlockLabel, usize>,
    branch_dispatch_blocks: &HashSet<BlockLabel>,
    resume_case_states: &HashMap<TrustedOwnerResumeCaseKey, TrustedOwnerState>,
    out_states: &[Option<TrustedOwnerState>],
    module_constants: &[ConstantExpr],
) -> Vec<TrustedOwnerState> {
    if edge.branch_gate.is_some() && branch_dispatch_blocks.contains(&edge.from) {
        if let Some(key) = edge
            .branch_gate
            .as_ref()
            .and_then(trusted_owner_resume_case_key)
            && let Some(state) = resume_case_states.get(&key)
        {
            return vec![remap_trusted_owner_state_for_edge(
                edge.explicit_local_remaps.as_deref(),
                state,
            )];
        }
        let dispatch_edges = predecessors
            .get(&edge.from)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut states = Vec::new();
        for dispatch_edge in dispatch_edges {
            let Some(source_index) = labels.get(&dispatch_edge.from).copied() else {
                continue;
            };
            let Some(source_state) = out_states[source_index].as_ref() else {
                continue;
            };
            let dispatch_state = remap_trusted_owner_state_for_edge(
                dispatch_edge.explicit_local_remaps.as_deref(),
                source_state,
            );
            if !trusted_owner_branch_gate_allows_state(
                edge.branch_gate.as_ref(),
                &dispatch_state,
                module_constants,
            ) {
                continue;
            }
            states.push(remap_trusted_owner_state_for_edge(
                edge.explicit_local_remaps.as_deref(),
                &dispatch_state,
            ));
        }
        return states;
    }

    let Some(source_index) = labels.get(&edge.from).copied() else {
        return Vec::new();
    };
    let Some(source_state) = out_states[source_index].as_ref() else {
        return Vec::new();
    };
    if !trusted_owner_branch_gate_allows_state(
        edge.branch_gate.as_ref(),
        source_state,
        module_constants,
    ) {
        return Vec::new();
    }
    vec![remap_trusted_owner_state_for_edge(
        edge.explicit_local_remaps.as_deref(),
        source_state,
    )]
}

fn trusted_owner_in_state_for_block(
    block_label: BlockLabel,
    entry_label: BlockLabel,
    predecessors: &HashMap<BlockLabel, Vec<TrustedOwnerPredecessorEdge>>,
    labels: &HashMap<BlockLabel, usize>,
    branch_dispatch_blocks: &HashSet<BlockLabel>,
    resume_case_states: &HashMap<TrustedOwnerResumeCaseKey, TrustedOwnerState>,
    out_states: &[Option<TrustedOwnerState>],
    module_constants: &[ConstantExpr],
) -> Option<TrustedOwnerState> {
    if block_label == entry_label {
        return Some(TrustedOwnerState::default());
    }
    let edges = predecessors
        .get(&block_label)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    match edges {
        [] => None,
        [edge] => trusted_owner_states_for_edge(
            edge,
            predecessors,
            labels,
            branch_dispatch_blocks,
            resume_case_states,
            out_states,
            module_constants,
        )
        .into_iter()
        .next(),
        _ => {
            let incoming = edges
                .iter()
                .flat_map(|edge| {
                    trusted_owner_states_for_edge(
                        edge,
                        predecessors,
                        labels,
                        branch_dispatch_blocks,
                        resume_case_states,
                        out_states,
                        module_constants,
                    )
                })
                .collect::<Vec<_>>();
            match incoming.len() {
                0 => None,
                1 => incoming.into_iter().next(),
                _ => Some(merge_trusted_owner_states(&incoming)),
            }
        }
    }
}

fn transfer_trusted_function_instr(instr: &InstrTyped, state: &mut TrustedOwnerState) {
    match instr {
        InstrTyped::Store(store) => {
            if let Some(location) = store.name.local_location() {
                if let Some(function_id) = trusted_function_id_for_expr(store.value.as_ref(), state)
                {
                    state.local_functions.insert(location, function_id);
                } else {
                    state.local_functions.remove(&location);
                }
            }
            if let Some(location) = store.name.preserved_location() {
                if let Some(function_id) = trusted_function_id_for_expr(store.value.as_ref(), state)
                {
                    state.preserved_functions.insert(location, function_id);
                } else {
                    state.preserved_functions.remove(&location);
                }
            }
        }
        InstrTyped::Del(del) => {
            if let Some(location) = del.name.local_location() {
                state.local_functions.remove(&location);
            }
            if let Some(location) = del.name.preserved_location() {
                state.preserved_functions.remove(&location);
            }
        }
        _ => {}
    }
}

fn merge_trusted_function_states(states: &[TrustedOwnerState]) -> TrustedOwnerState {
    let Some(first) = states.first() else {
        return TrustedOwnerState::default();
    };
    TrustedOwnerState {
        local_functions: first
            .local_functions
            .iter()
            .filter(|(location, function_id)| {
                states
                    .iter()
                    .all(|state| state.local_functions.get(location) == Some(*function_id))
            })
            .map(|(location, function_id)| (*location, *function_id))
            .collect(),
        preserved_functions: first
            .preserved_functions
            .iter()
            .filter(|(location, function_id)| {
                states
                    .iter()
                    .all(|state| state.preserved_functions.get(location) == Some(*function_id))
            })
            .map(|(location, function_id)| (*location, *function_id))
            .collect(),
        ..TrustedOwnerState::default()
    }
}

fn remap_trusted_function_state_for_edge(
    explicit_local_remaps: Option<&[(LocalLocation, LocalLocation)]>,
    state: &TrustedOwnerState,
) -> TrustedOwnerState {
    let Some(local_remaps) = explicit_local_remaps else {
        return state.clone();
    };
    let mut remapped = state.clone();
    for (source, target) in local_remaps {
        match remapped.local_functions.get(source).copied() {
            Some(function_id) => {
                remapped.local_functions.insert(*target, function_id);
            }
            None => {
                remapped.local_functions.remove(target);
            }
        }
    }
    remapped
}

pub fn analyze_trusted_function_states(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> TrustedOwnerStateAnalysis {
    let labels = function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.label, index))
        .collect::<HashMap<_, _>>();
    let mut predecessors = trusted_owner_block_predecessor_edges(function);
    let local_locations_by_name = typed_local_locations_by_name(function);
    for (target_label, edges) in predecessors.iter_mut() {
        let Some(target_index) = labels.get(target_label).copied() else {
            continue;
        };
        let target = &function.blocks[target_index];
        for edge in edges {
            edge.explicit_local_remaps = edge.explicit_args.as_deref().map(|args| {
                trusted_owner_local_remaps_for_edge(target, args, &local_locations_by_name)
            });
        }
    }
    let Some(entry_label) = function.blocks.first().map(|block| block.label) else {
        return TrustedOwnerStateAnalysis::default();
    };
    let mut successors = vec![Vec::<usize>::new(); function.blocks.len()];
    for (target, edges) in &predecessors {
        let Some(target_index) = labels.get(target).copied() else {
            continue;
        };
        for edge in edges {
            let Some(source_index) = labels.get(&edge.from).copied() else {
                continue;
            };
            successors[source_index].push(target_index);
        }
    }
    let mut in_states = vec![None::<TrustedOwnerState>; function.blocks.len()];
    let mut out_states = vec![None::<TrustedOwnerState>; function.blocks.len()];
    let entry_index = labels[&entry_label];
    let mut pending = VecDeque::from([entry_index]);
    let mut queued = vec![false; function.blocks.len()];
    queued[entry_index] = true;

    while let Some(block_index) = pending.pop_front() {
        queued[block_index] = false;
        let block = &function.blocks[block_index];
        let in_state = if block.label == entry_label {
            Some(TrustedOwnerState::default())
        } else {
            let edges = predecessors
                .get(&block.label)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            match edges {
                [] => None,
                [edge] => labels.get(&edge.from).copied().and_then(|source_index| {
                    out_states[source_index].as_ref().map(|source_state| {
                        remap_trusted_function_state_for_edge(
                            edge.explicit_local_remaps.as_deref(),
                            source_state,
                        )
                    })
                }),
                _ => {
                    let incoming = edges
                        .iter()
                        .filter_map(|edge| {
                            let source_index = *labels.get(&edge.from)?;
                            let source_state = out_states[source_index].as_ref()?;
                            Some(remap_trusted_function_state_for_edge(
                                edge.explicit_local_remaps.as_deref(),
                                source_state,
                            ))
                        })
                        .collect::<Vec<_>>();
                    match incoming.len() {
                        0 => None,
                        1 => incoming.into_iter().next(),
                        _ => Some(merge_trusted_function_states(&incoming)),
                    }
                }
            }
        };
        if in_states[block_index] == in_state && out_states[block_index].is_some() {
            continue;
        }
        in_states[block_index] = in_state.clone();
        let out_state = in_state.map(|mut state| {
            for instr in &block.body {
                transfer_trusted_function_instr(instr, &mut state);
            }
            state
        });
        if out_states[block_index] == out_state {
            continue;
        }
        out_states[block_index] = out_state;
        for successor_index in successors[block_index].iter().copied() {
            if !queued[successor_index] {
                pending.push_back(successor_index);
                queued[successor_index] = true;
            }
        }
    }

    let mut analysis = TrustedOwnerStateAnalysis::default();
    for (block_index, block) in function.blocks.iter().enumerate() {
        let Some(mut state) = in_states[block_index].clone() else {
            continue;
        };
        for (instr_index, instr) in block.body.iter().enumerate() {
            analysis.body_before_instr.insert(
                TypedVirtualBodyInstr {
                    block: block.label,
                    instr_index,
                },
                state.clone(),
            );
            transfer_trusted_function_instr(instr, &mut state);
        }
        analysis.block_before_term.insert(block.label, state);
    }
    analysis
}

pub fn analyze_trusted_owner_states(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
) -> TrustedOwnerStateAnalysis {
    let total_start = Instant::now();
    let labels = function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.label, index))
        .collect::<HashMap<_, _>>();
    let mut predecessors = trusted_owner_block_predecessor_edges(function);
    let branch_dispatch_blocks = function
        .blocks
        .iter()
        .filter(|block| block.body.is_empty() && matches!(block.term, BlockTerm::BranchTable(_)))
        .map(|block| block.label)
        .collect::<HashSet<_>>();
    let local_locations_by_name = typed_local_locations_by_name(function);
    for (target_label, edges) in predecessors.iter_mut() {
        let Some(target_index) = labels.get(target_label).copied() else {
            continue;
        };
        let target = &function.blocks[target_index];
        for edge in edges {
            edge.explicit_local_remaps = edge.explicit_args.as_deref().map(|args| {
                trusted_owner_local_remaps_for_edge(target, args, &local_locations_by_name)
            });
        }
    }
    let Some(entry_label) = function.blocks.first().map(|block| block.label) else {
        return TrustedOwnerStateAnalysis::default();
    };
    let mut successors = vec![Vec::<usize>::new(); function.blocks.len()];
    let mut resume_case_targets = HashMap::<TrustedOwnerResumeCaseKey, Vec<usize>>::new();
    for (target, edges) in &predecessors {
        let Some(target_index) = labels.get(target).copied() else {
            continue;
        };
        for edge in edges {
            let Some(source_index) = labels.get(&edge.from).copied() else {
                continue;
            };
            successors[source_index].push(target_index);
            if branch_dispatch_blocks.contains(&edge.from)
                && let Some(key) = edge
                    .branch_gate
                    .as_ref()
                    .and_then(trusted_owner_resume_case_key)
            {
                resume_case_targets
                    .entry(key)
                    .or_default()
                    .push(target_index);
            }
        }
    }
    let mut out_states = vec![None::<TrustedOwnerState>; function.blocks.len()];
    let mut resume_case_states = HashMap::<TrustedOwnerResumeCaseKey, TrustedOwnerState>::new();
    let entry_index = labels[&entry_label];
    let mut pending = VecDeque::from([entry_index]);
    let mut queued = vec![false; function.blocks.len()];
    queued[entry_index] = true;

    let dataflow_start = Instant::now();
    while let Some(block_index) = pending.pop_front() {
        queued[block_index] = false;
        let block = &function.blocks[block_index];
        let in_state = trusted_owner_in_state_for_block(
            block.label,
            entry_label,
            &predecessors,
            &labels,
            &branch_dispatch_blocks,
            &resume_case_states,
            &out_states,
            module_constants,
        );
        let out_state = if let Some(mut state) = in_state {
            let mut pending_resume_case_keys =
                HashMap::<LocalLocation, TrustedOwnerResumeCaseKey>::new();
            for instr in &block.body {
                let resume_case_key = match instr {
                    InstrTyped::Store(store) => store.name.local_location().and_then(|location| {
                        let value = trusted_i64_value_for_expr(
                            store.value.as_ref(),
                            &state,
                            module_constants,
                        )?;
                        let key = (location, value);
                        resume_case_targets.contains_key(&key).then_some(key)
                    }),
                    _ => None,
                };
                transfer_trusted_owner_instr(
                    instr,
                    &mut state,
                    module_constants,
                    trusted_constructor_calls,
                    trusted_constructor_init_owners,
                );
                if let Some(key @ (location, _)) = resume_case_key {
                    pending_resume_case_keys.insert(location, key);
                }
            }
            for key in pending_resume_case_keys.into_values() {
                if merge_trusted_owner_resume_case_state(
                    &mut resume_case_states,
                    key,
                    state.clone(),
                ) && let Some(targets) = resume_case_targets.get(&key)
                {
                    for target_index in targets.iter().copied() {
                        if !queued[target_index] {
                            pending.push_back(target_index);
                            queued[target_index] = true;
                        }
                    }
                }
            }
            Some(state)
        } else {
            None
        };
        if out_states[block_index] == out_state {
            continue;
        }
        out_states[block_index] = out_state;
        for successor_index in successors[block_index].iter().copied() {
            if !queued[successor_index] {
                pending.push_back(successor_index);
                queued[successor_index] = true;
            }
        }
    }
    let dataflow_elapsed = dataflow_start.elapsed();

    let materialize_start = Instant::now();
    let mut analysis = TrustedOwnerStateAnalysis::default();
    for block in &function.blocks {
        let Some(mut state) = trusted_owner_in_state_for_block(
            block.label,
            entry_label,
            &predecessors,
            &labels,
            &branch_dispatch_blocks,
            &resume_case_states,
            &out_states,
            module_constants,
        ) else {
            continue;
        };
        for (instr_index, instr) in block.body.iter().enumerate() {
            analysis.body_before_instr.insert(
                TypedVirtualBodyInstr {
                    block: block.label,
                    instr_index,
                },
                state.clone(),
            );
            transfer_trusted_owner_instr(
                instr,
                &mut state,
                module_constants,
                trusted_constructor_calls,
                trusted_constructor_init_owners,
            );
        }
        analysis.block_before_term.insert(block.label, state);
    }
    let materialize_elapsed = materialize_start.elapsed();
    tracing::info!(
        target: "soac_jit_codegen",
        event = "soac.trusted_owner_analysis_detail",
        function_id = ?function.function_id,
        function_qualname = %function.names.qualname,
        block_count = u64::try_from(function.blocks.len()).unwrap_or(u64::MAX),
        body_state_count = u64::try_from(analysis.body_before_instr.len()).unwrap_or(u64::MAX),
        term_state_count = u64::try_from(analysis.block_before_term.len()).unwrap_or(u64::MAX),
        dataflow_us = duration_micros(dataflow_elapsed),
        materialize_us = duration_micros(materialize_elapsed),
        total_us = duration_micros(total_start.elapsed()),
        "trusted_owner_analysis_detail",
    );
    analysis
}

pub fn visit_trusted_owner_term_instrs(
    term: &BlockTerm<InstrTyped>,
    visitor: &mut impl Visit<InstrTyped>,
) {
    match term {
        BlockTerm::IfTerm(if_term) => visitor.visit_instr(&if_term.test),
        BlockTerm::BranchTable(branch) => visitor.visit_instr(&branch.index),
        BlockTerm::Raise(raise) => {
            if let Some(exc) = &raise.exc {
                visitor.visit_instr(exc);
            }
        }
        BlockTerm::Return(value) => visitor.visit_instr(value),
        BlockTerm::Jump(_) => {}
    }
}
