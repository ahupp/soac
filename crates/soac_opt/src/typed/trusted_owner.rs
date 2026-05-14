use super::*;
use soac_core::block_py::{AbruptKind, FunctionKind};
use soac_ir_typed::{ProvenanceFact, TypedGeneratorInstancePlan, TypedGeneratorResumePlan};
use std::borrow::Cow;
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
    pub object_origin_candidates: HashMap<LocalLocation, HashSet<InstrId>>,
    pub preserved_object_origin_candidates: HashMap<PreservedLocation, HashSet<InstrId>>,
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
    pub origins: TrustedResumeFunctionOrigins,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrustedResumeFunctionOrigins {
    Single(InstrId),
    Multiple(HashSet<InstrId>),
}

impl TrustedResumeFunctionOrigins {
    fn single(origin: InstrId) -> Self {
        Self::Single(origin)
    }

    fn exact_origin(&self) -> Option<InstrId> {
        match self {
            Self::Single(origin) => Some(*origin),
            Self::Multiple(_) => None,
        }
    }

    fn sorted_vec(&self) -> Vec<InstrId> {
        let mut origins = match self {
            Self::Single(origin) => vec![*origin],
            Self::Multiple(origins) => origins.iter().copied().collect(),
        };
        origins.sort_by_key(|origin| origin.index());
        origins
    }

    fn extend_from(&mut self, other: &Self) -> bool {
        match other {
            Self::Single(right) => match self {
                Self::Single(left) if left == right => false,
                Self::Single(left) => {
                    let left = *left;
                    *self = Self::Multiple(HashSet::from([left, *right]));
                    true
                }
                Self::Multiple(left) => left.insert(*right),
            },
            Self::Multiple(right) => match self {
                Self::Single(left) => {
                    let left = *left;
                    let mut widened = right.clone();
                    widened.insert(left);
                    *self = Self::Multiple(widened);
                    true
                }
                Self::Multiple(left) => {
                    let before = left.len();
                    left.extend(right.iter().copied());
                    before != left.len()
                }
            },
        }
    }
}

impl TrustedResumeFunctionFact {
    fn new(function_id: RuntimeFunctionId, origin: InstrId) -> Self {
        Self {
            function_id,
            origins: TrustedResumeFunctionOrigins::single(origin),
        }
    }

    fn exact_origin(&self) -> Option<InstrId> {
        self.origins.exact_origin()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrustedOwnerStateAnalysis {
    pub body_before_instr: HashMap<TypedVirtualBodyInstr, TrustedOwnerState>,
    pub body_before_instr_components: HashMap<TypedVirtualBodyInstr, Vec<TrustedOwnerState>>,
    pub block_before_term: HashMap<BlockLabel, TrustedOwnerState>,
    pub block_before_term_components: HashMap<BlockLabel, Vec<TrustedOwnerState>>,
    pub reachable_blocks: TypedReachableBlockView,
}

#[derive(Clone, Debug)]
pub struct TrustedOwnerPredecessorEdge {
    pub from: BlockLabel,
    pub explicit_args: Option<Vec<BlockArg>>,
    pub explicit_local_remaps: Option<Vec<(LocalLocation, LocalLocation)>>,
    pub explicit_i64_values: Option<Vec<(LocalLocation, i64)>>,
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum TrustedOwnerResumeCaseLocation {
    Local(LocalLocation),
    Preserved(PreservedLocation),
}

type TrustedOwnerResumeCaseKey = (TrustedOwnerResumeCaseLocation, i64);
type TrustedOwnerDispatchCaseStateKey = (BlockLabel, TrustedOwnerResumeCaseKey);

#[derive(Clone, Debug, Default)]
struct TrustedOwnerResumeProtocolStates {
    resume_case_states: HashMap<TrustedOwnerResumeCaseKey, TrustedOwnerState>,
    abrupt_case_block_states: HashMap<BlockLabel, TrustedOwnerState>,
    abrupt_case_keys_by_block: HashMap<BlockLabel, HashSet<TrustedOwnerResumeCaseKey>>,
    abrupt_dispatch_case_states: HashMap<TrustedOwnerDispatchCaseStateKey, TrustedOwnerState>,
    released_case_in_states: HashMap<BlockLabel, TrustedOwnerState>,
}

#[derive(Clone, Debug, Default)]
struct TrustedOwnerDataflowStats {
    ordinary_edge_state_emissions: u64,
    ordinary_edge_merge_attempts: u64,
    ordinary_edge_merge_changes: u64,
    abrupt_case_pending_pops: u64,
    abrupt_case_edge_state_batches: u64,
    abrupt_case_edge_key_emissions: u64,
    abrupt_case_in_merge_attempts: u64,
    abrupt_case_in_merge_changes: u64,
    dispatch_case_merge_attempts: u64,
    dispatch_case_merge_changes: u64,
}

fn trusted_owner_resume_case_location_for_name(
    name: &ResolvedName,
) -> Option<TrustedOwnerResumeCaseLocation> {
    name.local_location()
        .map(TrustedOwnerResumeCaseLocation::Local)
        .or_else(|| {
            name.preserved_location()
                .map(TrustedOwnerResumeCaseLocation::Preserved)
        })
}

fn trusted_owner_resume_case_value_for_location(
    location: TrustedOwnerResumeCaseLocation,
    state: &TrustedOwnerState,
) -> Option<i64> {
    match location {
        TrustedOwnerResumeCaseLocation::Local(location) => state.i64_values.get(&location).copied(),
        TrustedOwnerResumeCaseLocation::Preserved(location) => {
            state.preserved_i64_values.get(&location).copied()
        }
    }
}

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

fn trusted_object_origin_candidates_for_name(
    name: &ResolvedName,
    state: &TrustedOwnerState,
) -> Option<Vec<InstrId>> {
    let candidates = if let Some(location) = name.local_location() {
        state.object_origin_candidates.get(&location)
    } else {
        state
            .preserved_object_origin_candidates
            .get(&name.preserved_location()?)
    };
    let Some(candidates) = candidates else {
        return trusted_object_origin_for_name(name, state).map(|origin| vec![origin]);
    };
    let mut candidates = candidates.iter().copied().collect::<Vec<_>>();
    candidates.sort_by_key(|origin| origin.index());
    Some(candidates)
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

fn trusted_function_id_for_store_value(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> Option<RuntimeFunctionId> {
    trusted_field_function_id_for_expr(expr, state, module_constants)
        .map(|(_, function_id)| function_id)
        .or_else(|| trusted_function_id_for_expr(expr, state))
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
    let function_id = trusted_function_field_target_for_origin(origin, field_name, state)?;
    Some((origin, function_id))
}

pub fn trusted_function_field_target_for_origin(
    origin: InstrId,
    field_name: &str,
    state: &TrustedOwnerState,
) -> Option<RuntimeFunctionId> {
    state
        .function_fields
        .get(&(origin, field_name.to_string()))
        .copied()
}

fn trusted_function_field_target_for_origin_candidates(
    candidate_origins: &[InstrId],
    field_name: &str,
    state: &TrustedOwnerState,
) -> Option<RuntimeFunctionId> {
    let mut targets = candidate_origins
        .iter()
        .map(|origin| trusted_function_field_target_for_origin(*origin, field_name, state));
    let target = targets.next()??;
    targets
        .all(|candidate| candidate == Some(target))
        .then_some(target)
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

pub fn trusted_generator_resume_function_fact_for_name<'a>(
    name: &ResolvedName,
    state: &'a TrustedOwnerState,
) -> Option<&'a TrustedResumeFunctionFact> {
    trusted_resume_function_fact_for_name(name, state)
}

pub fn trusted_generator_origin_for_name(
    name: &ResolvedName,
    state: &TrustedOwnerState,
) -> Option<InstrId> {
    trusted_object_origin_for_name(name, state)
}

pub fn trusted_generator_origin_has_escaped(origin: InstrId, state: &TrustedOwnerState) -> bool {
    state.escaped_origins.contains(&origin)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TrustedGeneratorResumeTargetLookup {
    Present {
        resume_origin: Option<InstrId>,
        function_id: RuntimeFunctionId,
        candidate_origins: Vec<InstrId>,
    },
    Missing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TrustedGeneratorResumeSiteFacts {
    function_id: RuntimeFunctionId,
    candidate_origins: Vec<InstrId>,
    generator_origin: Option<InstrId>,
    disposition: TrustedGeneratorResumeSiteDisposition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TrustedGeneratorResumeSiteLookup {
    Present(TrustedGeneratorResumeSiteFacts),
    MissingResumeFunction {
        resume_temp_fact: Option<TrustedResumeFunctionFact>,
        owner_fact: Option<TrustedResumeFunctionFact>,
    },
    MissingOwnerOrigin {
        resume_origin: InstrId,
        function_id: RuntimeFunctionId,
    },
    OriginMismatch {
        generator_origin: InstrId,
        resume_origin: InstrId,
        function_id: RuntimeFunctionId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrustedGeneratorResumeSiteDisposition {
    Ready,
    Escaped { generator_origin: InstrId },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrustedGeneratorResumePlanLookup {
    Present {
        instr_id: InstrId,
        plan: TypedGeneratorResumePlan,
    },
    RejectedBecauseEscaped {
        instr_id: InstrId,
        plan: TypedGeneratorResumePlan,
        generator_origin: InstrId,
    },
    Missing {
        instr_id: Option<InstrId>,
        reason: TrustedGeneratorResumePlanMissReason,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TrustedGeneratorResumePlanMissReason {
    MissingInstrId,
    UnsupportedExpr,
    UntrustedResumeName,
    Keywords,
    ArgShape,
    NonLoadOwner,
    MissingResumeFunction,
    MissingOwnerOrigin,
    OriginMismatch,
}

fn trusted_generator_resume_fact_target(
    fact: &TrustedResumeFunctionFact,
) -> (Option<InstrId>, RuntimeFunctionId, Vec<InstrId>) {
    let candidate_origins = fact.origins.sorted_vec();
    (fact.exact_origin(), fact.function_id, candidate_origins)
}

fn trusted_generator_resume_call_target(
    resume_function: &InstrTyped,
    owner: &Load<InstrTyped>,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> TrustedGeneratorResumeTargetLookup {
    if let Some((origin, function_id)) =
        trusted_field_function_id_for_expr(resume_function, state, module_constants)
    {
        return TrustedGeneratorResumeTargetLookup::Present {
            resume_origin: Some(origin),
            function_id,
            candidate_origins: vec![origin],
        };
    }
    if let Some(function_id) = trusted_function_id_for_expr(resume_function, state)
        && let Some(origin) = trusted_object_origin_for_name(&owner.name, state)
        && trusted_function_field_target_for_origin(origin, "_resume_function", state)
            == Some(function_id)
    {
        return TrustedGeneratorResumeTargetLookup::Present {
            resume_origin: Some(origin),
            function_id,
            candidate_origins: vec![origin],
        };
    }
    if let Some(candidate_origins) = trusted_object_origin_candidates_for_name(&owner.name, state)
        && let Some(function_id) = trusted_function_field_target_for_origin_candidates(
            &candidate_origins,
            "_resume_function",
            state,
        )
    {
        return TrustedGeneratorResumeTargetLookup::Present {
            resume_origin: None,
            function_id,
            candidate_origins,
        };
    }
    if let InstrTyped::Load(load) = resume_function
        && let Some(fact) = trusted_resume_function_fact_for_name(&load.name, state)
    {
        let (resume_origin, function_id, candidate_origins) =
            trusted_generator_resume_fact_target(fact);
        return TrustedGeneratorResumeTargetLookup::Present {
            resume_origin,
            function_id,
            candidate_origins,
        };
    }
    let Some(fact) = trusted_resume_function_fact_for_name(&owner.name, state) else {
        return TrustedGeneratorResumeTargetLookup::Missing;
    };
    let (resume_origin, function_id, candidate_origins) =
        trusted_generator_resume_fact_target(fact);
    TrustedGeneratorResumeTargetLookup::Present {
        resume_origin,
        function_id,
        candidate_origins,
    }
}

fn trusted_generator_resume_site_lookup(
    resume_function: &InstrTyped,
    owner: &Load<InstrTyped>,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> TrustedGeneratorResumeSiteLookup {
    let TrustedGeneratorResumeTargetLookup::Present {
        resume_origin,
        function_id,
        candidate_origins,
    } = trusted_generator_resume_call_target(resume_function, owner, state, module_constants)
    else {
        let resume_temp_fact = match resume_function {
            InstrTyped::Load(load) => {
                trusted_generator_resume_function_fact_for_name(&load.name, state).cloned()
            }
            _ => None,
        };
        let owner_fact =
            trusted_generator_resume_function_fact_for_name(&owner.name, state).cloned();
        return TrustedGeneratorResumeSiteLookup::MissingResumeFunction {
            resume_temp_fact,
            owner_fact,
        };
    };
    let generator_origin = trusted_generator_origin_for_name(&owner.name, state);
    match (resume_origin, generator_origin) {
        (Some(resume_origin), Some(generator_origin)) if resume_origin != generator_origin => {
            return TrustedGeneratorResumeSiteLookup::OriginMismatch {
                generator_origin,
                resume_origin,
                function_id,
            };
        }
        (Some(resume_origin), None) => {
            return TrustedGeneratorResumeSiteLookup::MissingOwnerOrigin {
                resume_origin,
                function_id,
            };
        }
        _ => {}
    }
    let disposition = generator_origin
        .into_iter()
        .chain(candidate_origins.iter().copied())
        .find(|origin| trusted_generator_origin_has_escaped(*origin, state))
        .map_or(
            TrustedGeneratorResumeSiteDisposition::Ready,
            |generator_origin| TrustedGeneratorResumeSiteDisposition::Escaped { generator_origin },
        );
    TrustedGeneratorResumeSiteLookup::Present(TrustedGeneratorResumeSiteFacts {
        function_id,
        candidate_origins,
        generator_origin,
        disposition,
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

fn trusted_runtime_iterator_owner_type(owner_type_ref: &TypedAttrOwnerRef) -> bool {
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
    if !trusted_runtime_iterator_owner_type(&owner_type_ref) {
        return None;
    }
    let origin = trusted_object_origin_for_name(&receiver.name, state)?;
    Some((owner_type_ref, origin))
}

fn trusted_runtime_next_receiver_is_internal_consumption(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> bool {
    let InstrTyped::CallTyped(call) = expr else {
        return false;
    };
    let is_next_call = match &call.access {
        soac_ir_typed::TypedCallAccessPlan::Generic => {
            typed_expr_is_runtime_name_load(call.func.as_ref(), RuntimeName::Next, module_constants)
        }
        soac_ir_typed::TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
            runtime_name, ..
        } => *runtime_name == RuntimeName::Next,
        _ => false,
    };
    if !is_next_call || !call.keywords.is_empty() {
        return false;
    }
    let Some(CallArgPositional::Positional(InstrTyped::Load(receiver))) = call.args.first() else {
        return false;
    };
    let Some(owner_type_ref) = trusted_owner_state_for_name(&receiver.name, state) else {
        return false;
    };
    trusted_runtime_iterator_owner_type(owner_type_ref)
        && trusted_object_origin_for_name(&receiver.name, state).is_some()
}

fn trusted_generator_state_reader_is_internal_observation(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> bool {
    let (func, args, has_keywords) = match expr {
        InstrTyped::CallTyped(call) => (
            call.func.as_ref(),
            call.args.as_slice(),
            !call.keywords.is_empty(),
        ),
        InstrTyped::GuardedCallableCallTyped(call) => (
            call.func.as_ref(),
            call.args.as_slice(),
            !call.keywords.is_empty(),
        ),
        InstrTyped::DirectCallableCallTyped(call) => {
            (call.func.as_ref(), call.args.as_slice(), false)
        }
        _ => return false,
    };
    if has_keywords
        || !matches!(
            trusted_runtime_name_for_expr(func, state, module_constants),
            Some(
                RuntimeName::IsGeneratorClosed
                    | RuntimeName::CurrentYieldfrom
                    | RuntimeName::CurrentThrowContext
            )
        )
    {
        return false;
    }
    let Some(CallArgPositional::Positional(InstrTyped::Load(receiver))) = args.first() else {
        return false;
    };
    trusted_object_origin_for_name(&receiver.name, state).is_some()
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
    if let Some(candidate_origins) = value
        .typed_extra()
        .and_then(TypedInstrExtra::trusted_object_origin_candidates)
        && let [origin] = candidate_origins
    {
        return Some(*origin);
    }
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

fn trusted_object_origin_candidates_for_store_value(
    value: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
) -> Option<HashSet<InstrId>> {
    if let Some(candidate_origins) = value
        .typed_extra()
        .and_then(TypedInstrExtra::trusted_object_origin_candidates)
    {
        return Some(candidate_origins.iter().copied().collect());
    }
    if let Some((_, origin)) = trusted_identity_iter_store_value(value, state, module_constants) {
        return Some(HashSet::from([origin]));
    }
    if value.generator_instance_plan().is_some() {
        return Some(HashSet::from([value.try_semantic_instr_id()?]));
    }
    if let Some(instr_id) = value.try_semantic_instr_id()
        && trusted_constructor_calls.contains_key(&instr_id)
    {
        return Some(HashSet::from([instr_id]));
    }
    let InstrTyped::Load(load) = value else {
        return None;
    };
    trusted_object_origin_candidates_for_name(&load.name, state)
        .map(|candidates| candidates.into_iter().collect())
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
    module_constants: &[ConstantExpr],
) -> Option<TrustedResumeFunctionFact> {
    if let Some(function_id) = value
        .typed_extra()
        .and_then(TypedInstrExtra::trusted_generator_resume_function)
        && let Some(candidate_origins) = value
            .typed_extra()
            .and_then(TypedInstrExtra::trusted_object_origin_candidates)
    {
        let mut origins = candidate_origins.iter().copied();
        let first = origins.next()?;
        let mut fact = TrustedResumeFunctionFact::new(function_id, first);
        for origin in origins {
            fact.origins
                .extend_from(&TrustedResumeFunctionOrigins::single(origin));
        }
        return Some(fact);
    }
    if let Some(plan) = value.generator_instance_plan() {
        return Some(TrustedResumeFunctionFact::new(
            plan.function_id,
            value.try_semantic_instr_id()?,
        ));
    }
    if let Some((origin, function_id)) =
        trusted_field_function_id_for_expr(value, state, module_constants)
    {
        return Some(TrustedResumeFunctionFact::new(function_id, origin));
    }
    if let InstrTyped::GetAttrTyped(get_attr) = value
        && typed_constant_string(get_attr.attr.as_ref(), module_constants)
            == Some("_resume_function")
        && let InstrTyped::Load(receiver) = get_attr.value.as_ref()
        && let Some(origin) = trusted_object_origin_for_name(&receiver.name, state)
        && let Some(function_id) =
            trusted_function_field_target_for_origin(origin, "_resume_function", state)
    {
        return Some(TrustedResumeFunctionFact::new(function_id, origin));
    }
    if let InstrTyped::GetAttrTyped(get_attr) = value
        && typed_constant_string(get_attr.attr.as_ref(), module_constants)
            == Some("_resume_function")
        && let InstrTyped::Load(receiver) = get_attr.value.as_ref()
        && let Some(candidate_origins) =
            trusted_object_origin_candidates_for_name(&receiver.name, state)
        && let Some(function_id) = trusted_function_field_target_for_origin_candidates(
            &candidate_origins,
            "_resume_function",
            state,
        )
    {
        let mut origins = candidate_origins.into_iter();
        let first = origins.next()?;
        let mut fact = TrustedResumeFunctionFact::new(function_id, first);
        for origin in origins {
            fact.origins
                .extend_from(&TrustedResumeFunctionOrigins::single(origin));
        }
        return Some(fact);
    }
    let InstrTyped::Load(load) = value else {
        return None;
    };
    trusted_resume_function_fact_for_name(&load.name, state).cloned()
}

fn trusted_generator_boundary_attr_load(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> bool {
    let InstrTyped::GetAttrTyped(get_attr) = expr else {
        return false;
    };
    if !matches!(
        typed_constant_string(get_attr.attr.as_ref(), module_constants),
        Some("_resume_function" | "_preserved_values")
    ) {
        return false;
    }
    let InstrTyped::Load(receiver) = get_attr.value.as_ref() else {
        return false;
    };
    let Some(origin) = trusted_object_origin_for_name(&receiver.name, state) else {
        return false;
    };
    trusted_function_field_target_for_origin(origin, "_resume_function", state).is_some()
}

fn trusted_generator_boundary_attr_store_value(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> bool {
    let InstrTyped::GetAttrTyped(get_attr) = expr else {
        return false;
    };
    if !matches!(
        typed_constant_string(get_attr.attr.as_ref(), module_constants),
        Some("_resume_function" | "_preserved_values")
    ) {
        return false;
    }
    let InstrTyped::Load(receiver) = get_attr.value.as_ref() else {
        return false;
    };
    trusted_object_origin_for_name(&receiver.name, state).is_some()
}

fn trusted_generator_protocol_method_attr_load(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> bool {
    let InstrTyped::GetAttrTyped(get_attr) = expr else {
        return false;
    };
    if !matches!(
        typed_constant_string(get_attr.attr.as_ref(), module_constants),
        Some("__iter__" | "__next__" | "send")
    ) {
        return false;
    }
    let InstrTyped::Load(receiver) = get_attr.value.as_ref() else {
        return false;
    };
    let Some(owner) = trusted_owner_state_for_name(&receiver.name, state) else {
        return false;
    };
    matches!(
        owner,
        TypedAttrOwnerRef::TypeKey {
            module_name,
            qualname,
        } if module_name == "soac.runtime" && qualname == "ClosureGenerator"
    ) && trusted_object_origin_for_name(&receiver.name, state).is_some()
}

fn trusted_generator_resume_plan_lookup_from_parts(
    instr_id: InstrId,
    func: &InstrTyped,
    args: &[CallArgPositional<InstrTyped>],
    has_keywords: bool,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> TrustedGeneratorResumePlanLookup {
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
        return TrustedGeneratorResumePlanLookup::Missing {
            instr_id: Some(instr_id),
            reason: TrustedGeneratorResumePlanMissReason::UntrustedResumeName,
        };
    }
    if has_keywords {
        tracing::debug!(
            target: "soac_generator_resume_planning",
            instr_id = ?instr_id,
            "typed_generator_resume_plan_skipped_keywords",
        );
        return TrustedGeneratorResumePlanLookup::Missing {
            instr_id: Some(instr_id),
            reason: TrustedGeneratorResumePlanMissReason::Keywords,
        };
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
        return TrustedGeneratorResumePlanLookup::Missing {
            instr_id: Some(instr_id),
            reason: TrustedGeneratorResumePlanMissReason::ArgShape,
        };
    };
    let InstrTyped::Load(owner) = owner else {
        tracing::debug!(
            target: "soac_generator_resume_planning",
            instr_id = ?instr_id,
            owner = ?owner,
            "typed_generator_resume_plan_skipped_non_load_owner",
        );
        return TrustedGeneratorResumePlanLookup::Missing {
            instr_id: Some(instr_id),
            reason: TrustedGeneratorResumePlanMissReason::NonLoadOwner,
        };
    };
    let TrustedGeneratorResumeSiteFacts {
        function_id,
        candidate_origins,
        generator_origin,
        disposition,
    } = match trusted_generator_resume_site_lookup(resume_function, owner, state, module_constants)
    {
        TrustedGeneratorResumeSiteLookup::Present(facts) => facts,
        TrustedGeneratorResumeSiteLookup::MissingResumeFunction {
            resume_temp_fact,
            owner_fact,
        } => {
            let owner_origin = trusted_generator_origin_for_name(&owner.name, state);
            let owner_resume_target = owner_origin.and_then(|origin| {
                trusted_function_field_target_for_origin(origin, "_resume_function", state)
            });
            let owner_origin_candidates =
                trusted_object_origin_candidates_for_name(&owner.name, state);
            let resume_name = match resume_function {
                InstrTyped::Load(load) => Some(load.name.id_str()),
                _ => None,
            };
            eprintln!(
                "missing trusted resume function instr_id={instr_id:?} resume_name={resume_name:?} resume_function={resume_function:?} owner={} owner_origin={owner_origin:?} owner_origin_candidates={owner_origin_candidates:?} owner_resume_target={owner_resume_target:?} resume_temp_fact={resume_temp_fact:?} owner_fact={owner_fact:?}",
                owner.name.id_str(),
            );
            tracing::debug!(
                target: "soac_generator_resume_planning",
                instr_id = ?instr_id,
                resume_function = ?resume_function,
                owner_name = owner.name.id_str(),
                resume_temp_fact = ?resume_temp_fact,
                owner_fact = ?owner_fact,
                "typed_generator_resume_plan_skipped_missing_resume_function",
            );
            return TrustedGeneratorResumePlanLookup::Missing {
                instr_id: Some(instr_id),
                reason: TrustedGeneratorResumePlanMissReason::MissingResumeFunction,
            };
        }
        TrustedGeneratorResumeSiteLookup::MissingOwnerOrigin {
            resume_origin,
            function_id,
        } => {
            tracing::debug!(
                target: "soac_generator_resume_planning",
                instr_id = ?instr_id,
                owner_name = owner.name.id_str(),
                resume_origin = ?resume_origin,
                function_id = ?function_id,
                "typed_generator_resume_plan_skipped_missing_owner_origin",
            );
            return TrustedGeneratorResumePlanLookup::Missing {
                instr_id: Some(instr_id),
                reason: TrustedGeneratorResumePlanMissReason::MissingOwnerOrigin,
            };
        }
        TrustedGeneratorResumeSiteLookup::OriginMismatch {
            generator_origin,
            resume_origin,
            function_id,
        } => {
            tracing::debug!(
                target: "soac_generator_resume_planning",
                instr_id = ?instr_id,
                generator_origin = ?generator_origin,
                resume_origin = ?resume_origin,
                function_id = ?function_id,
                "typed_generator_resume_plan_skipped_origin_mismatch",
            );
            return TrustedGeneratorResumePlanLookup::Missing {
                instr_id: Some(instr_id),
                reason: TrustedGeneratorResumePlanMissReason::OriginMismatch,
            };
        }
    };
    tracing::debug!(
        target: "soac_generator_resume_planning",
        instr_id = ?instr_id,
        generator_origin = ?generator_origin,
        function_id = ?function_id,
        "typed_generator_resume_plan_selected",
    );
    let plan = TypedGeneratorResumePlan {
        function_id,
        generator_origin,
        candidate_origins,
    };
    match disposition {
        TrustedGeneratorResumeSiteDisposition::Escaped { generator_origin } => {
            TrustedGeneratorResumePlanLookup::RejectedBecauseEscaped {
                instr_id,
                plan,
                generator_origin,
            }
        }
        TrustedGeneratorResumeSiteDisposition::Ready => {
            TrustedGeneratorResumePlanLookup::Present { instr_id, plan }
        }
    }
}

pub fn trusted_generator_resume_plan_lookup_for_expr(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> TrustedGeneratorResumePlanLookup {
    let Some(instr_id) = expr.try_semantic_instr_id() else {
        return TrustedGeneratorResumePlanLookup::Missing {
            instr_id: None,
            reason: TrustedGeneratorResumePlanMissReason::MissingInstrId,
        };
    };
    match expr {
        InstrTyped::CallTyped(call) => trusted_generator_resume_plan_lookup_from_parts(
            instr_id,
            call.func.as_ref(),
            &call.args,
            !call.keywords.is_empty(),
            state,
            module_constants,
        ),
        InstrTyped::GuardedCallableCallTyped(call) => {
            trusted_generator_resume_plan_lookup_from_parts(
                instr_id,
                call.func.as_ref(),
                &call.args,
                !call.keywords.is_empty(),
                state,
                module_constants,
            )
        }
        InstrTyped::DirectCallableCallTyped(call) => {
            trusted_generator_resume_plan_lookup_from_parts(
                instr_id,
                call.func.as_ref(),
                &call.args,
                false,
                state,
                module_constants,
            )
        }
        _ => TrustedGeneratorResumePlanLookup::Missing {
            instr_id: Some(instr_id),
            reason: TrustedGeneratorResumePlanMissReason::UnsupportedExpr,
        },
    }
}

fn trusted_generator_resume_lookup_recognizes_call(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> bool {
    matches!(
        trusted_generator_resume_plan_lookup_for_expr(expr, state, module_constants),
        TrustedGeneratorResumePlanLookup::Present { .. }
            | TrustedGeneratorResumePlanLookup::RejectedBecauseEscaped { .. }
    )
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
            if trusted_generator_boundary_attr_load(expr, self.state, self.module_constants) {
                return;
            }
            if trusted_generator_protocol_method_attr_load(expr, self.state, self.module_constants)
            {
                return;
            }
            if trusted_generator_resume_lookup_recognizes_call(
                expr,
                self.state,
                self.module_constants,
            ) {
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
            if trusted_runtime_next_receiver_is_internal_consumption(
                expr,
                self.state,
                self.module_constants,
            ) {
                let InstrTyped::CallTyped(call) = expr else {
                    unreachable!("trusted next consumption must come from a typed call")
                };
                for arg in call.args.iter().skip(1) {
                    self.visit_call_arg(arg);
                }
                return;
            }
            if trusted_generator_state_reader_is_internal_observation(
                expr,
                self.state,
                self.module_constants,
            ) {
                match expr {
                    InstrTyped::CallTyped(call) => {
                        for arg in call.args.iter().skip(1) {
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
                        for arg in call.args.iter().skip(1) {
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
                        for arg in call.args.iter().skip(1) {
                            self.visit_call_arg(arg);
                        }
                    }
                    _ => unreachable!(
                        "trusted generator state reader must come from a call expression"
                    ),
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
    let escape_debug_enabled = tracing::enabled!(
        target: "soac_generator_resume_planning",
        tracing::Level::DEBUG
    );
    let escaped_before = escape_debug_enabled.then(|| state.escaped_origins.clone());
    let escaped = match instr {
        InstrTyped::Store(store)
            if (store.name.local_location().is_some()
                || store.name.preserved_location().is_some())
                && matches!(store.value.as_ref(), InstrTyped::Load(_)) =>
        {
            HashSet::new()
        }
        InstrTyped::Store(store)
            if trusted_identity_iter_store_value(store.value.as_ref(), state, module_constants)
                .is_some() =>
        {
            HashSet::new()
        }
        InstrTyped::Store(store)
            if trusted_generator_boundary_attr_store_value(
                store.value.as_ref(),
                state,
                module_constants,
            ) =>
        {
            HashSet::new()
        }
        InstrTyped::Store(store)
            if trusted_generator_protocol_method_attr_load(
                store.value.as_ref(),
                state,
                module_constants,
            ) =>
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
    if let Some(escaped_before) = escaped_before {
        let newly_escaped = state
            .escaped_origins
            .difference(&escaped_before)
            .copied()
            .collect::<Vec<_>>();
        if !newly_escaped.is_empty() {
            let escape_site_kind = match instr {
                InstrTyped::Store(_) => "store",
                InstrTyped::SetAttrTyped(_) => "set_attr",
                InstrTyped::CallTyped(_) => "call",
                InstrTyped::GuardedCallableCallTyped(_) => "guarded_call",
                InstrTyped::DirectCallableCallTyped(_) => "direct_call",
                _ => "other",
            };
            tracing::debug!(
                target: "soac_generator_resume_planning",
                escape_site_kind,
                instr_id = ?instr.try_semantic_instr_id(),
                newly_escaped = ?newly_escaped,
                "typed_trusted_owner_generator_origins_marked_escaped",
            );
        }
    }
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
                let resume_function = trusted_resume_function_for_store_value(
                    store.value.as_ref(),
                    state,
                    module_constants,
                );
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
                if let Some(candidate_origins) = trusted_object_origin_candidates_for_store_value(
                    store.value.as_ref(),
                    state,
                    module_constants,
                    trusted_constructor_calls,
                ) {
                    state
                        .object_origin_candidates
                        .insert(location, candidate_origins);
                } else {
                    state.object_origin_candidates.remove(&location);
                }
                if let Some(fact) = resume_function {
                    state.resume_functions.insert(location, fact);
                } else {
                    state.resume_functions.remove(&location);
                }
                if let Some(function_id) = trusted_function_id_for_store_value(
                    store.value.as_ref(),
                    state,
                    module_constants,
                ) {
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
                let resume_function = trusted_resume_function_for_store_value(
                    store.value.as_ref(),
                    state,
                    module_constants,
                );
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
                if let Some(candidate_origins) = trusted_object_origin_candidates_for_store_value(
                    store.value.as_ref(),
                    state,
                    module_constants,
                    trusted_constructor_calls,
                ) {
                    state
                        .preserved_object_origin_candidates
                        .insert(location, candidate_origins);
                } else {
                    state.preserved_object_origin_candidates.remove(&location);
                }
                if let Some(fact) = resume_function {
                    state.preserved_resume_functions.insert(location, fact);
                } else {
                    state.preserved_resume_functions.remove(&location);
                }
                if let Some(function_id) = trusted_function_id_for_store_value(
                    store.value.as_ref(),
                    state,
                    module_constants,
                ) {
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
                state.object_origin_candidates.remove(&location);
                state.local_functions.remove(&location);
                state.resume_functions.remove(&location);
            }
            if let Some(location) = del.name.preserved_location() {
                state.preserved_owners.remove(&location);
                state.preserved_runtime_names.remove(&location);
                state.preserved_i64_values.remove(&location);
                state.preserved_object_origins.remove(&location);
                state.preserved_object_origin_candidates.remove(&location);
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

fn merge_trusted_owner_state_refs(states: &[&TrustedOwnerState]) -> TrustedOwnerState {
    let Some(first) = states.first() else {
        return TrustedOwnerState::default();
    };
    if states.len() == 1 {
        return (*first).clone();
    }
    if let [left, right] = states {
        return merge_trusted_owner_state_pair(left, right);
    }
    let mut merged = (*first).clone();
    for state in states.iter().skip(1) {
        merged
            .locals
            .retain(|location, owner| state.locals.get(location) == Some(owner));
        merged
            .preserved_owners
            .retain(|location, owner| state.preserved_owners.get(location) == Some(owner));
        merged.runtime_names.retain(|location, runtime_name| {
            state.runtime_names.get(location) == Some(runtime_name)
        });
        merged
            .i64_values
            .retain(|location, value| state.i64_values.get(location) == Some(value));
        merged
            .preserved_runtime_names
            .retain(|location, runtime_name| {
                state.preserved_runtime_names.get(location) == Some(runtime_name)
            });
        merged
            .preserved_i64_values
            .retain(|location, value| state.preserved_i64_values.get(location) == Some(value));
        merged
            .object_origins
            .retain(|location, origin| state.object_origins.get(location) == Some(origin));
        merged.preserved_object_origins.retain(|location, origin| {
            state.preserved_object_origins.get(location) == Some(origin)
        });
        merged.object_origin_candidates.retain(|location, origins| {
            if let Some(other) = state.object_origin_candidates.get(location) {
                origins.extend(other.iter().copied());
            }
            true
        });
        merged
            .preserved_object_origin_candidates
            .retain(|location, origins| {
                if let Some(other) = state.preserved_object_origin_candidates.get(location) {
                    origins.extend(other.iter().copied());
                }
                true
            });
        merged.local_functions.retain(|location, function_id| {
            state.local_functions.get(location) == Some(function_id)
        });
        merged.preserved_functions.retain(|location, function_id| {
            state.preserved_functions.get(location) == Some(function_id)
        });
        merged
            .escaped_origins
            .extend(state.escaped_origins.iter().copied());
    }
    let mut conflicting_resume_functions = HashSet::new();
    let mut conflicting_preserved_resume_functions = HashSet::new();
    for state in states.iter().skip(1) {
        for (location, fact) in &state.resume_functions {
            if conflicting_resume_functions.contains(location) {
                continue;
            }
            match merged.resume_functions.entry(*location) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(fact.clone());
                }
                std::collections::hash_map::Entry::Occupied(mut entry)
                    if entry.get().function_id == fact.function_id =>
                {
                    entry.get_mut().origins.extend_from(&fact.origins);
                }
                std::collections::hash_map::Entry::Occupied(entry) => {
                    entry.remove();
                    conflicting_resume_functions.insert(*location);
                }
            }
        }
        for (location, fact) in &state.preserved_resume_functions {
            if conflicting_preserved_resume_functions.contains(location) {
                continue;
            }
            match merged.preserved_resume_functions.entry(*location) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(fact.clone());
                }
                std::collections::hash_map::Entry::Occupied(mut entry)
                    if entry.get().function_id == fact.function_id =>
                {
                    entry.get_mut().origins.extend_from(&fact.origins);
                }
                std::collections::hash_map::Entry::Occupied(entry) => {
                    entry.remove();
                    conflicting_preserved_resume_functions.insert(*location);
                }
            }
        }
    }
    let mut conflicting_function_fields = HashSet::new();
    for state in states.iter().skip(1) {
        for (field, function_id) in &state.function_fields {
            if conflicting_function_fields.contains(field) {
                continue;
            }
            match merged.function_fields.entry(field.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(*function_id);
                }
                std::collections::hash_map::Entry::Occupied(entry)
                    if entry.get() == function_id => {}
                std::collections::hash_map::Entry::Occupied(entry) => {
                    entry.remove();
                    conflicting_function_fields.insert(field.clone());
                }
            }
        }
    }
    merged
}

fn merge_trusted_owner_state_pair(
    left: &TrustedOwnerState,
    right: &TrustedOwnerState,
) -> TrustedOwnerState {
    let mut merged = left.clone();
    merge_trusted_owner_state_pair_in_place(&mut merged, right);
    merged
}

fn merge_trusted_owner_state_pair_in_place(
    merged: &mut TrustedOwnerState,
    right: &TrustedOwnerState,
) -> bool {
    let mut changed = false;
    let before = merged.locals.len();
    merged
        .locals
        .retain(|location, owner| right.locals.get(location) == Some(owner));
    changed |= before != merged.locals.len();
    let before = merged.preserved_owners.len();
    merged
        .preserved_owners
        .retain(|location, owner| right.preserved_owners.get(location) == Some(owner));
    changed |= before != merged.preserved_owners.len();
    let before = merged.runtime_names.len();
    merged
        .runtime_names
        .retain(|location, runtime_name| right.runtime_names.get(location) == Some(runtime_name));
    changed |= before != merged.runtime_names.len();
    let before = merged.i64_values.len();
    merged
        .i64_values
        .retain(|location, value| right.i64_values.get(location) == Some(value));
    changed |= before != merged.i64_values.len();
    let before = merged.preserved_runtime_names.len();
    merged
        .preserved_runtime_names
        .retain(|location, runtime_name| {
            right.preserved_runtime_names.get(location) == Some(runtime_name)
        });
    changed |= before != merged.preserved_runtime_names.len();
    let before = merged.preserved_i64_values.len();
    merged
        .preserved_i64_values
        .retain(|location, value| right.preserved_i64_values.get(location) == Some(value));
    changed |= before != merged.preserved_i64_values.len();
    let before = merged.object_origins.len();
    merged
        .object_origins
        .retain(|location, origin| right.object_origins.get(location) == Some(origin));
    changed |= before != merged.object_origins.len();
    let before = merged.preserved_object_origins.len();
    merged
        .preserved_object_origins
        .retain(|location, origin| right.preserved_object_origins.get(location) == Some(origin));
    changed |= before != merged.preserved_object_origins.len();
    let before = merged.object_origin_candidates.clone();
    merged.object_origin_candidates.retain(|location, origins| {
        if let Some(other) = right.object_origin_candidates.get(location) {
            origins.extend(other.iter().copied());
        }
        true
    });
    changed |= before != merged.object_origin_candidates;
    let before = merged.preserved_object_origin_candidates.clone();
    merged
        .preserved_object_origin_candidates
        .retain(|location, origins| {
            if let Some(other) = right.preserved_object_origin_candidates.get(location) {
                origins.extend(other.iter().copied());
            }
            true
        });
    changed |= before != merged.preserved_object_origin_candidates;
    let before = merged.local_functions.len();
    merged
        .local_functions
        .retain(|location, function_id| right.local_functions.get(location) == Some(function_id));
    changed |= before != merged.local_functions.len();
    let before = merged.preserved_functions.len();
    merged.preserved_functions.retain(|location, function_id| {
        right.preserved_functions.get(location) == Some(function_id)
    });
    changed |= before != merged.preserved_functions.len();
    for (location, fact) in &right.resume_functions {
        match merged.resume_functions.entry(*location) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(fact.clone());
                changed = true;
            }
            std::collections::hash_map::Entry::Occupied(mut entry)
                if entry.get().function_id == fact.function_id =>
            {
                changed |= entry.get_mut().origins.extend_from(&fact.origins);
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                entry.remove();
                changed = true;
            }
        }
    }
    for (location, fact) in &right.preserved_resume_functions {
        match merged.preserved_resume_functions.entry(*location) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(fact.clone());
                changed = true;
            }
            std::collections::hash_map::Entry::Occupied(mut entry)
                if entry.get().function_id == fact.function_id =>
            {
                changed |= entry.get_mut().origins.extend_from(&fact.origins);
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                entry.remove();
                changed = true;
            }
        }
    }
    for (field, function_id) in &right.function_fields {
        match merged.function_fields.entry(field.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(*function_id);
                changed = true;
            }
            std::collections::hash_map::Entry::Occupied(entry) if entry.get() == function_id => {}
            std::collections::hash_map::Entry::Occupied(entry) => {
                entry.remove();
                changed = true;
            }
        }
    }
    let before = merged.escaped_origins.len();
    merged
        .escaped_origins
        .extend(right.escaped_origins.iter().copied());
    changed |= before != merged.escaped_origins.len();
    changed
}

pub fn merge_trusted_owner_states(states: &[TrustedOwnerState]) -> TrustedOwnerState {
    let state_refs = states.iter().collect::<Vec<_>>();
    merge_trusted_owner_state_refs(&state_refs)
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
                        explicit_i64_values: None,
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
                            explicit_i64_values: None,
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
                            explicit_i64_values: None,
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
                        explicit_i64_values: None,
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
                    explicit_i64_values: None,
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

fn trusted_owner_abrupt_kind_tag(kind: AbruptKind) -> i64 {
    match kind {
        AbruptKind::Fallthrough => 0,
        AbruptKind::Return => 1,
        AbruptKind::Exception => 2,
        AbruptKind::Break => 3,
        AbruptKind::Continue => 4,
    }
}

fn trusted_owner_i64_values_for_edge(
    target: &soac_ir_typed::TypedBlock,
    explicit_args: &[BlockArg],
    local_locations_by_name: &HashMap<String, LocalLocation>,
) -> Vec<(LocalLocation, i64)> {
    target
        .params
        .iter()
        .zip(explicit_args)
        .filter_map(|(param, arg)| {
            if param.role != BlockParamRole::AbruptKind {
                return None;
            }
            let BlockArg::AbruptKind(kind) = arg else {
                return None;
            };
            Some((
                local_locations_by_name.get(&param.name).copied()?,
                trusted_owner_abrupt_kind_tag(*kind),
            ))
        })
        .collect()
}

pub fn remap_trusted_owner_state_for_edge(
    explicit_local_remaps: Option<&[(LocalLocation, LocalLocation)]>,
    explicit_i64_values: Option<&[(LocalLocation, i64)]>,
    state: &TrustedOwnerState,
) -> TrustedOwnerState {
    remap_trusted_owner_state_for_edge_borrowed(explicit_local_remaps, explicit_i64_values, state)
        .into_owned()
}

fn remap_trusted_owner_state_for_edge_borrowed<'a>(
    explicit_local_remaps: Option<&[(LocalLocation, LocalLocation)]>,
    explicit_i64_values: Option<&[(LocalLocation, i64)]>,
    state: &'a TrustedOwnerState,
) -> Cow<'a, TrustedOwnerState> {
    if explicit_local_remaps.is_none_or(<[_]>::is_empty)
        && explicit_i64_values.is_none_or(<[_]>::is_empty)
    {
        return Cow::Borrowed(state);
    }
    let mut remapped = state.clone();
    if let Some(local_remaps) = explicit_local_remaps {
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
            match remapped.object_origin_candidates.get(source).cloned() {
                Some(origins) => {
                    remapped.object_origin_candidates.insert(*target, origins);
                }
                None => {
                    remapped.object_origin_candidates.remove(target);
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
    }
    if let Some(explicit_i64_values) = explicit_i64_values {
        for (target, value) in explicit_i64_values {
            remapped.locals.remove(target);
            remapped.runtime_names.remove(target);
            remapped.object_origins.remove(target);
            remapped.object_origin_candidates.remove(target);
            remapped.resume_functions.remove(target);
            remapped.local_functions.remove(target);
            remapped.i64_values.insert(*target, *value);
        }
    }
    Cow::Owned(remapped)
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
        trusted_owner_resume_case_location_for_name(index_name)?,
        i64::try_from(*case_index).ok()?,
    ))
}

fn merge_trusted_owner_resume_case_state(
    states: &mut HashMap<TrustedOwnerResumeCaseKey, TrustedOwnerState>,
    key: TrustedOwnerResumeCaseKey,
    next: TrustedOwnerState,
) -> bool {
    let Some(existing) = states.get_mut(&key) else {
        states.insert(key, next);
        return true;
    };
    merge_trusted_owner_state_pair_in_place(existing, &next)
}

fn merge_trusted_owner_dispatch_case_state(
    states: &mut HashMap<TrustedOwnerDispatchCaseStateKey, TrustedOwnerState>,
    block_label: BlockLabel,
    key: TrustedOwnerResumeCaseKey,
    next: TrustedOwnerState,
) -> bool {
    let state_key = (block_label, key);
    let Some(existing) = states.get_mut(&state_key) else {
        states.insert(state_key, next);
        return true;
    };
    merge_trusted_owner_state_pair_in_place(existing, &next)
}

fn merge_trusted_owner_block_state(
    states: &mut HashMap<BlockLabel, TrustedOwnerState>,
    block_label: BlockLabel,
    next: TrustedOwnerState,
) -> bool {
    let Some(existing) = states.get_mut(&block_label) else {
        states.insert(block_label, next);
        return true;
    };
    merge_trusted_owner_state_pair_in_place(existing, &next)
}

fn queue_trusted_owner_abrupt_case_block_state(
    states: &mut HashMap<BlockLabel, TrustedOwnerState>,
    keys_by_block: &mut HashMap<BlockLabel, HashSet<TrustedOwnerResumeCaseKey>>,
    pending: &mut VecDeque<BlockLabel>,
    queued: &mut HashSet<BlockLabel>,
    stats: &mut TrustedOwnerDataflowStats,
    block_label: BlockLabel,
    key: TrustedOwnerResumeCaseKey,
    next: TrustedOwnerState,
) -> bool {
    stats.abrupt_case_in_merge_attempts += 1;
    let state_changed = merge_trusted_owner_block_state(states, block_label, next);
    let key_inserted = keys_by_block.entry(block_label).or_default().insert(key);
    if !state_changed && !key_inserted {
        return false;
    }
    stats.abrupt_case_in_merge_changes += 1;
    if queued.insert(block_label) {
        pending.push_back(block_label);
    }
    true
}

fn trusted_owner_case_key_after_edge(
    edge: &TrustedOwnerPredecessorEdge,
    key: TrustedOwnerResumeCaseKey,
    state: &TrustedOwnerState,
) -> Option<TrustedOwnerResumeCaseKey> {
    let (mut location, value) = key;
    if let Some(local_remaps) = edge.explicit_local_remaps.as_deref() {
        for (source, target) in local_remaps {
            if location == TrustedOwnerResumeCaseLocation::Local(*source) {
                location = TrustedOwnerResumeCaseLocation::Local(*target);
            }
        }
    }
    (trusted_owner_resume_case_value_for_location(location, state) == Some(value))
        .then_some((location, value))
}

fn trusted_owner_case_key_alias_after_store(
    instr: &InstrTyped,
    key: TrustedOwnerResumeCaseKey,
    state: &TrustedOwnerState,
) -> Option<TrustedOwnerResumeCaseKey> {
    let InstrTyped::Store(store) = instr else {
        return None;
    };
    let InstrTyped::Load(source) = store.value.as_ref() else {
        return None;
    };
    let (source_location, value) = key;
    if trusted_owner_resume_case_location_for_name(&source.name)? != source_location {
        return None;
    }
    if trusted_owner_resume_case_value_for_location(source_location, state) != Some(value) {
        return None;
    }
    Some((
        trusted_owner_resume_case_location_for_name(&store.name)?,
        value,
    ))
}

fn trusted_owner_abrupt_case_states_for_edge(
    edge: &TrustedOwnerPredecessorEdge,
    source_state: &TrustedOwnerState,
) -> Vec<(TrustedOwnerResumeCaseKey, TrustedOwnerState)> {
    let Some(explicit_i64_values) = edge.explicit_i64_values.as_deref() else {
        return Vec::new();
    };
    if explicit_i64_values.is_empty() {
        return Vec::new();
    }
    let remapped = remap_trusted_owner_state_for_edge(
        edge.explicit_local_remaps.as_deref(),
        edge.explicit_i64_values.as_deref(),
        source_state,
    );
    explicit_i64_values
        .iter()
        .copied()
        .filter(|(location, value)| remapped.i64_values.get(location).copied() == Some(*value))
        .map(|(location, value)| {
            (
                (TrustedOwnerResumeCaseLocation::Local(location), value),
                remapped.clone(),
            )
        })
        .collect()
}

fn trusted_owner_states_for_edge<'a>(
    edge: &TrustedOwnerPredecessorEdge,
    predecessors: &HashMap<BlockLabel, Vec<TrustedOwnerPredecessorEdge>>,
    labels: &HashMap<BlockLabel, usize>,
    branch_dispatch_blocks: &HashSet<BlockLabel>,
    dispatch_case_states: &'a HashMap<TrustedOwnerDispatchCaseStateKey, TrustedOwnerState>,
    resume_case_states: &'a HashMap<TrustedOwnerResumeCaseKey, TrustedOwnerState>,
    out_states: &'a [Option<TrustedOwnerState>],
    module_constants: &[ConstantExpr],
) -> Vec<Cow<'a, TrustedOwnerState>> {
    if edge.branch_gate.is_some() && branch_dispatch_blocks.contains(&edge.from) {
        let matching_dispatch_case_states = edge
            .branch_gate
            .as_ref()
            .into_iter()
            .flat_map(|gate| -> Vec<&TrustedOwnerState> {
                if let Some(key) = trusted_owner_resume_case_key(gate) {
                    return dispatch_case_states
                        .get(&(edge.from, key))
                        .into_iter()
                        .collect::<Vec<_>>();
                }
                dispatch_case_states
                    .iter()
                    .filter(move |((block_label, _), state)| {
                        *block_label == edge.from
                            && trusted_owner_branch_gate_allows_state(
                                Some(gate),
                                state,
                                module_constants,
                            )
                    })
                    .map(|(_, state)| state)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        if !matching_dispatch_case_states.is_empty() {
            return matching_dispatch_case_states
                .into_iter()
                .map(|state| {
                    remap_trusted_owner_state_for_edge_borrowed(
                        edge.explicit_local_remaps.as_deref(),
                        edge.explicit_i64_values.as_deref(),
                        state,
                    )
                })
                .collect();
        }
        let has_dispatch_case_family = edge
            .branch_gate
            .as_ref()
            .and_then(|gate| match gate {
                TrustedOwnerBranchGate::Case { index_name, .. }
                | TrustedOwnerBranchGate::Default { index_name, .. } => {
                    trusted_owner_resume_case_location_for_name(index_name)
                }
            })
            .is_some_and(|dispatch_location| {
                dispatch_case_states.keys().any(|(block_label, case_key)| {
                    *block_label == edge.from && case_key.0 == dispatch_location
                })
            });
        if has_dispatch_case_family {
            return Vec::new();
        }
        if let Some(key) = edge
            .branch_gate
            .as_ref()
            .and_then(trusted_owner_resume_case_key)
            && let Some(state) = resume_case_states.get(&key)
        {
            return vec![remap_trusted_owner_state_for_edge_borrowed(
                edge.explicit_local_remaps.as_deref(),
                edge.explicit_i64_values.as_deref(),
                state,
            )];
        }
        if edge
            .branch_gate
            .as_ref()
            .and_then(trusted_owner_resume_case_key)
            .is_some()
            && let Some(source_index) = labels.get(&edge.from).copied()
            && let Some(source_state) = out_states[source_index].as_ref()
            && trusted_owner_branch_gate_allows_state(
                edge.branch_gate.as_ref(),
                source_state,
                module_constants,
            )
        {
            return vec![remap_trusted_owner_state_for_edge_borrowed(
                edge.explicit_local_remaps.as_deref(),
                edge.explicit_i64_values.as_deref(),
                source_state,
            )];
        }
        let dispatch_edges = predecessors
            .get(&edge.from)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if dispatch_edges.is_empty()
            && let Some(source_index) = labels.get(&edge.from).copied()
            && let Some(source_state) = out_states[source_index].as_ref()
            && trusted_owner_branch_gate_allows_state(
                edge.branch_gate.as_ref(),
                source_state,
                module_constants,
            )
        {
            return vec![remap_trusted_owner_state_for_edge_borrowed(
                edge.explicit_local_remaps.as_deref(),
                edge.explicit_i64_values.as_deref(),
                source_state,
            )];
        }
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
                dispatch_edge.explicit_i64_values.as_deref(),
                source_state,
            );
            if !trusted_owner_branch_gate_allows_state(
                edge.branch_gate.as_ref(),
                &dispatch_state,
                module_constants,
            ) {
                continue;
            }
            states.push(Cow::Owned(remap_trusted_owner_state_for_edge(
                edge.explicit_local_remaps.as_deref(),
                edge.explicit_i64_values.as_deref(),
                &dispatch_state,
            )));
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
    vec![remap_trusted_owner_state_for_edge_borrowed(
        edge.explicit_local_remaps.as_deref(),
        edge.explicit_i64_values.as_deref(),
        source_state,
    )]
}

fn trusted_owner_materialized_in_state_for_block(
    block_label: BlockLabel,
    labels: &HashMap<BlockLabel, usize>,
    ordinary_in_states: &[Option<TrustedOwnerState>],
    resume_protocol_states: &TrustedOwnerResumeProtocolStates,
) -> Option<TrustedOwnerState> {
    let components = trusted_owner_materialized_in_state_components_for_block(
        block_label,
        labels,
        ordinary_in_states,
        resume_protocol_states,
    );
    match components.len() {
        0 => None,
        1 => components.into_iter().next(),
        _ => Some(merge_trusted_owner_states(&components)),
    }
}

fn trusted_owner_materialized_in_state_components_for_block(
    block_label: BlockLabel,
    labels: &HashMap<BlockLabel, usize>,
    ordinary_in_states: &[Option<TrustedOwnerState>],
    resume_protocol_states: &TrustedOwnerResumeProtocolStates,
) -> Vec<TrustedOwnerState> {
    let mut components = Vec::new();
    if let Some(state) = labels
        .get(&block_label)
        .and_then(|block_index| ordinary_in_states.get(*block_index))
        .and_then(Option::clone)
    {
        components.push(state);
    }
    if let Some(state) = resume_protocol_states
        .released_case_in_states
        .get(&block_label)
        .cloned()
    {
        components.push(state);
    }
    if let Some(state) = resume_protocol_states
        .abrupt_case_block_states
        .get(&block_label)
        .cloned()
    {
        components.push(state);
    }
    components
}

fn merge_trusted_owner_incremental_block_state(
    ordinary_in_states: &mut [Option<TrustedOwnerState>],
    target_index: usize,
    next: TrustedOwnerState,
) -> bool {
    let Some(existing) = ordinary_in_states[target_index].as_mut() else {
        ordinary_in_states[target_index] = Some(next);
        return true;
    };
    merge_trusted_owner_state_pair_in_place(existing, &next)
}

fn queue_trusted_owner_incremental_block_edge_states(
    ordinary_in_states: &mut [Option<TrustedOwnerState>],
    pending: &mut VecDeque<usize>,
    queued: &mut [bool],
    stats: &mut TrustedOwnerDataflowStats,
    target_index: usize,
    edge: &TrustedOwnerPredecessorEdge,
    predecessors: &HashMap<BlockLabel, Vec<TrustedOwnerPredecessorEdge>>,
    labels: &HashMap<BlockLabel, usize>,
    branch_dispatch_blocks: &HashSet<BlockLabel>,
    resume_protocol_states: &TrustedOwnerResumeProtocolStates,
    out_states: &[Option<TrustedOwnerState>],
    module_constants: &[ConstantExpr],
) -> bool {
    let mut changed = false;
    for state in trusted_owner_states_for_edge(
        edge,
        predecessors,
        labels,
        branch_dispatch_blocks,
        &resume_protocol_states.abrupt_dispatch_case_states,
        &resume_protocol_states.resume_case_states,
        out_states,
        module_constants,
    ) {
        stats.ordinary_edge_state_emissions += 1;
        stats.ordinary_edge_merge_attempts += 1;
        let state_changed = merge_trusted_owner_incremental_block_state(
            ordinary_in_states,
            target_index,
            state.into_owned(),
        );
        if state_changed {
            stats.ordinary_edge_merge_changes += 1;
        }
        changed |= state_changed;
    }
    if changed && !queued[target_index] {
        pending.push_back(target_index);
        queued[target_index] = true;
    }
    changed
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
    let reachable = TypedReachableBlockView::for_function(function);
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
            edge.explicit_i64_values = edge.explicit_args.as_deref().map(|args| {
                trusted_owner_i64_values_for_edge(target, args, &local_locations_by_name)
            });
        }
    }
    let Some(entry_label) = function.blocks.first().map(|block| block.label) else {
        return TrustedOwnerStateAnalysis::default();
    };
    let mut successors = vec![Vec::<usize>::new(); function.blocks.len()];
    for (target, edges) in &predecessors {
        if !reachable.contains(*target) {
            continue;
        }
        let Some(target_index) = labels.get(target).copied() else {
            continue;
        };
        for edge in edges {
            if !reachable.contains(edge.from) {
                continue;
            }
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
    analysis.reachable_blocks = reachable.clone();
    for (block_index, block) in function.blocks.iter().enumerate() {
        if !reachable.contains(block.label) {
            continue;
        }
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
    let reachable = TypedReachableBlockView::for_function(function);
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
        .filter(|block| matches!(block.term, BlockTerm::BranchTable(_)))
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
            edge.explicit_i64_values = edge.explicit_args.as_deref().map(|args| {
                trusted_owner_i64_values_for_edge(target, args, &local_locations_by_name)
            });
        }
    }
    let Some(entry_label) = function.blocks.first().map(|block| block.label) else {
        return TrustedOwnerStateAnalysis::default();
    };
    let mut successor_edges =
        vec![Vec::<(usize, TrustedOwnerPredecessorEdge)>::new(); function.blocks.len()];
    let mut resume_case_target_edges =
        HashMap::<TrustedOwnerResumeCaseKey, Vec<(usize, TrustedOwnerPredecessorEdge)>>::new();
    let mut dispatch_case_target_edges = HashMap::<
        TrustedOwnerDispatchCaseStateKey,
        Vec<(usize, TrustedOwnerPredecessorEdge)>,
    >::new();
    for (target, edges) in &predecessors {
        if !reachable.contains(*target) {
            continue;
        }
        let Some(target_index) = labels.get(target).copied() else {
            continue;
        };
        for edge in edges {
            if !reachable.contains(edge.from) {
                continue;
            }
            let Some(source_index) = labels.get(&edge.from).copied() else {
                continue;
            };
            successor_edges[source_index].push((target_index, edge.clone()));
            if branch_dispatch_blocks.contains(&edge.from)
                && let Some(key) = edge
                    .branch_gate
                    .as_ref()
                    .and_then(trusted_owner_resume_case_key)
            {
                resume_case_target_edges
                    .entry(key)
                    .or_default()
                    .push((target_index, edge.clone()));
                dispatch_case_target_edges
                    .entry((edge.from, key))
                    .or_default()
                    .push((target_index, edge.clone()));
            }
        }
    }
    let mut ordinary_in_states = vec![None::<TrustedOwnerState>; function.blocks.len()];
    let mut out_states = vec![None::<TrustedOwnerState>; function.blocks.len()];
    let mut resume_protocol_states = TrustedOwnerResumeProtocolStates::default();
    let mut dataflow_stats = TrustedOwnerDataflowStats::default();
    let entry_index = labels[&entry_label];
    ordinary_in_states[entry_index] = Some(TrustedOwnerState::default());
    let mut pending = VecDeque::from([entry_index]);
    let mut queued = vec![false; function.blocks.len()];
    let mut abrupt_case_pending = VecDeque::<BlockLabel>::new();
    let mut abrupt_case_queued = HashSet::<BlockLabel>::new();
    queued[entry_index] = true;

    let dataflow_start = Instant::now();
    while !pending.is_empty() || !abrupt_case_pending.is_empty() {
        let Some(block_index) = pending.pop_front() else {
            let block_label = abrupt_case_pending
                .pop_front()
                .expect("case pending queue should be non-empty");
            dataflow_stats.abrupt_case_pending_pops += 1;
            abrupt_case_queued.remove(&block_label);
            let Some(block_index) = labels.get(&block_label).copied() else {
                continue;
            };
            let Some(mut state) = resume_protocol_states
                .abrupt_case_block_states
                .get(&block_label)
                .cloned()
            else {
                continue;
            };
            let block = &function.blocks[block_index];
            let mut case_keys = resume_protocol_states
                .abrupt_case_keys_by_block
                .get(&block_label)
                .cloned()
                .unwrap_or_default();
            if case_keys.is_empty() {
                continue;
            }
            let mut pending_resume_case_keys =
                HashMap::<TrustedOwnerResumeCaseLocation, TrustedOwnerResumeCaseKey>::new();
            for instr in &block.body {
                let resume_case_key = match instr {
                    InstrTyped::Store(store) => trusted_owner_resume_case_location_for_name(
                        &store.name,
                    )
                    .and_then(|location| {
                        let value = trusted_i64_value_for_expr(
                            store.value.as_ref(),
                            &state,
                            module_constants,
                        )?;
                        let key = (location, value);
                        resume_case_target_edges.contains_key(&key).then_some(key)
                    }),
                    _ => None,
                };
                let copied_case_keys = case_keys
                    .iter()
                    .copied()
                    .filter_map(|key| trusted_owner_case_key_alias_after_store(instr, key, &state))
                    .collect::<Vec<_>>();
                transfer_trusted_owner_instr(
                    instr,
                    &mut state,
                    module_constants,
                    trusted_constructor_calls,
                    trusted_constructor_init_owners,
                );
                case_keys.extend(copied_case_keys.into_iter().filter(|(location, value)| {
                    trusted_owner_resume_case_value_for_location(*location, &state) == Some(*value)
                }));
                if let Some(key @ (location, _)) = resume_case_key {
                    pending_resume_case_keys.insert(location, key);
                }
            }
            for key in pending_resume_case_keys.into_values() {
                if merge_trusted_owner_resume_case_state(
                    &mut resume_protocol_states.resume_case_states,
                    key,
                    state.clone(),
                ) && let Some(targets) = resume_case_target_edges.get(&key)
                {
                    for (target_index, edge) in targets {
                        queue_trusted_owner_incremental_block_edge_states(
                            &mut ordinary_in_states,
                            &mut pending,
                            &mut queued,
                            &mut dataflow_stats,
                            *target_index,
                            edge,
                            &predecessors,
                            &labels,
                            &branch_dispatch_blocks,
                            &resume_protocol_states,
                            &out_states,
                            module_constants,
                        );
                    }
                }
            }
            if branch_dispatch_blocks.contains(&block.label) {
                for case_key in case_keys.iter().copied() {
                    dataflow_stats.dispatch_case_merge_attempts += 1;
                    let dispatch_state_changed = merge_trusted_owner_dispatch_case_state(
                        &mut resume_protocol_states.abrupt_dispatch_case_states,
                        block.label,
                        case_key,
                        state.clone(),
                    );
                    if dispatch_state_changed {
                        dataflow_stats.dispatch_case_merge_changes += 1;
                    }
                    if dispatch_state_changed
                        && let Some(targets) =
                            dispatch_case_target_edges.get(&(block.label, case_key))
                    {
                        for (target_index, edge) in targets {
                            queue_trusted_owner_incremental_block_edge_states(
                                &mut ordinary_in_states,
                                &mut pending,
                                &mut queued,
                                &mut dataflow_stats,
                                *target_index,
                                edge,
                                &predecessors,
                                &labels,
                                &branch_dispatch_blocks,
                                &resume_protocol_states,
                                &out_states,
                                module_constants,
                            );
                        }
                    }
                }
                for (target_index, edge) in &successor_edges[block_index] {
                    let remapped = remap_trusted_owner_state_for_edge(
                        edge.explicit_local_remaps.as_deref(),
                        edge.explicit_i64_values.as_deref(),
                        &state,
                    );
                    if !trusted_owner_branch_gate_allows_state(
                        edge.branch_gate.as_ref(),
                        &remapped,
                        module_constants,
                    ) {
                        continue;
                    }
                    let target_label = function.blocks[*target_index].label;
                    let next_case_keys = case_keys
                        .iter()
                        .copied()
                        .filter_map(|case_key| {
                            trusted_owner_case_key_after_edge(edge, case_key, &remapped)
                        })
                        .collect::<Vec<_>>();
                    if next_case_keys.is_empty()
                        && merge_trusted_owner_block_state(
                            &mut resume_protocol_states.released_case_in_states,
                            target_label,
                            remapped.clone(),
                        )
                        && !queued[*target_index]
                    {
                        pending.push_back(*target_index);
                        queued[*target_index] = true;
                    }
                    for next_case_key in next_case_keys {
                        queue_trusted_owner_abrupt_case_block_state(
                            &mut resume_protocol_states.abrupt_case_block_states,
                            &mut resume_protocol_states.abrupt_case_keys_by_block,
                            &mut abrupt_case_pending,
                            &mut abrupt_case_queued,
                            &mut dataflow_stats,
                            target_label,
                            next_case_key,
                            remapped.clone(),
                        );
                    }
                }
                continue;
            }
            for (target_index, edge) in &successor_edges[block_index] {
                let remapped = remap_trusted_owner_state_for_edge(
                    edge.explicit_local_remaps.as_deref(),
                    edge.explicit_i64_values.as_deref(),
                    &state,
                );
                if !trusted_owner_branch_gate_allows_state(
                    edge.branch_gate.as_ref(),
                    &remapped,
                    module_constants,
                ) {
                    continue;
                }
                let target_label = function.blocks[*target_index].label;
                let next_case_keys = case_keys
                    .iter()
                    .copied()
                    .filter_map(|case_key| {
                        trusted_owner_case_key_after_edge(edge, case_key, &remapped)
                    })
                    .collect::<Vec<_>>();
                if next_case_keys.is_empty()
                    && merge_trusted_owner_block_state(
                        &mut resume_protocol_states.released_case_in_states,
                        target_label,
                        remapped.clone(),
                    )
                    && !queued[*target_index]
                {
                    pending.push_back(*target_index);
                    queued[*target_index] = true;
                }
                for next_case_key in next_case_keys {
                    queue_trusted_owner_abrupt_case_block_state(
                        &mut resume_protocol_states.abrupt_case_block_states,
                        &mut resume_protocol_states.abrupt_case_keys_by_block,
                        &mut abrupt_case_pending,
                        &mut abrupt_case_queued,
                        &mut dataflow_stats,
                        target_label,
                        next_case_key,
                        remapped.clone(),
                    );
                }
            }
            continue;
        };
        queued[block_index] = false;
        let block = &function.blocks[block_index];
        let ordinary_state = ordinary_in_states[block_index].clone();
        let in_state = match (
            ordinary_state,
            resume_protocol_states
                .released_case_in_states
                .get(&block.label)
                .cloned(),
        ) {
            (None, None) => None,
            (Some(state), None) | (None, Some(state)) => Some(state),
            (Some(ordinary), Some(released)) => {
                Some(merge_trusted_owner_states(&[ordinary, released]))
            }
        };
        let out_state = if let Some(mut state) = in_state {
            let mut pending_resume_case_keys =
                HashMap::<TrustedOwnerResumeCaseLocation, TrustedOwnerResumeCaseKey>::new();
            for instr in &block.body {
                let resume_case_key = match instr {
                    InstrTyped::Store(store) => trusted_owner_resume_case_location_for_name(
                        &store.name,
                    )
                    .and_then(|location| {
                        let value = trusted_i64_value_for_expr(
                            store.value.as_ref(),
                            &state,
                            module_constants,
                        )?;
                        let key = (location, value);
                        resume_case_target_edges.contains_key(&key).then_some(key)
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
                    &mut resume_protocol_states.resume_case_states,
                    key,
                    state.clone(),
                ) && let Some(targets) = resume_case_target_edges.get(&key)
                {
                    for (target_index, edge) in targets {
                        queue_trusted_owner_incremental_block_edge_states(
                            &mut ordinary_in_states,
                            &mut pending,
                            &mut queued,
                            &mut dataflow_stats,
                            *target_index,
                            edge,
                            &predecessors,
                            &labels,
                            &branch_dispatch_blocks,
                            &resume_protocol_states,
                            &out_states,
                            module_constants,
                        );
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
        if let Some(source_state) = out_states[block_index].as_ref() {
            for (target_index, edge) in &successor_edges[block_index] {
                queue_trusted_owner_incremental_block_edge_states(
                    &mut ordinary_in_states,
                    &mut pending,
                    &mut queued,
                    &mut dataflow_stats,
                    *target_index,
                    edge,
                    &predecessors,
                    &labels,
                    &branch_dispatch_blocks,
                    &resume_protocol_states,
                    &out_states,
                    module_constants,
                );
                let abrupt_case_states =
                    trusted_owner_abrupt_case_states_for_edge(edge, source_state);
                if !abrupt_case_states.is_empty() {
                    dataflow_stats.abrupt_case_edge_state_batches += 1;
                    dataflow_stats.abrupt_case_edge_key_emissions +=
                        u64::try_from(abrupt_case_states.len()).unwrap_or(u64::MAX);
                }
                for (case_key, case_state) in abrupt_case_states {
                    let target_label = function.blocks[*target_index].label;
                    queue_trusted_owner_abrupt_case_block_state(
                        &mut resume_protocol_states.abrupt_case_block_states,
                        &mut resume_protocol_states.abrupt_case_keys_by_block,
                        &mut abrupt_case_pending,
                        &mut abrupt_case_queued,
                        &mut dataflow_stats,
                        target_label,
                        case_key,
                        case_state,
                    );
                }
            }
        }
    }
    let dataflow_elapsed = dataflow_start.elapsed();

    let materialize_start = Instant::now();
    let mut analysis = TrustedOwnerStateAnalysis::default();
    analysis.reachable_blocks = reachable.clone();
    for block in &function.blocks {
        if !reachable.contains(block.label) {
            continue;
        }
        let component_states = trusted_owner_materialized_in_state_components_for_block(
            block.label,
            &labels,
            &ordinary_in_states,
            &resume_protocol_states,
        );
        let Some(mut state) = trusted_owner_materialized_in_state_for_block(
            block.label,
            &labels,
            &ordinary_in_states,
            &resume_protocol_states,
        ) else {
            if tracing::enabled!(
                target: "soac_trusted_owner_materialize",
                tracing::Level::DEBUG
            ) {
                let predecessor_out_states = predecessors
                    .get(&block.label)
                    .map(|edges| {
                        edges
                            .iter()
                            .map(|edge| {
                                let has_out_state = labels
                                    .get(&edge.from)
                                    .and_then(|source_index| out_states.get(*source_index))
                                    .and_then(Option::as_ref)
                                    .is_some();
                                (edge.from, has_out_state)
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let abrupt_case_state_count = resume_protocol_states
                    .abrupt_case_keys_by_block
                    .get(&block.label)
                    .map(HashSet::len)
                    .unwrap_or(0);
                tracing::debug!(
                    target: "soac_trusted_owner_materialize",
                    function_id = ?function.function_id,
                    function_qualname = %function.names.qualname,
                    block = ?block.label,
                    predecessor_out_states = ?predecessor_out_states,
                    released_case_state_present = resume_protocol_states
                        .released_case_in_states
                        .contains_key(&block.label),
                    abrupt_case_state_count,
                    "trusted_owner_materialized_in_state_missing",
                );
            }
            continue;
        };
        let mut component_states_before_instr = component_states;
        for (instr_index, instr) in block.body.iter().enumerate() {
            let site = TypedVirtualBodyInstr {
                block: block.label,
                instr_index,
            };
            analysis.body_before_instr.insert(site, state.clone());
            if !component_states_before_instr.is_empty() {
                analysis
                    .body_before_instr_components
                    .insert(site, component_states_before_instr.clone());
            }
            transfer_trusted_owner_instr(
                instr,
                &mut state,
                module_constants,
                trusted_constructor_calls,
                trusted_constructor_init_owners,
            );
            for component_state in &mut component_states_before_instr {
                transfer_trusted_owner_instr(
                    instr,
                    component_state,
                    module_constants,
                    trusted_constructor_calls,
                    trusted_constructor_init_owners,
                );
            }
        }
        analysis.block_before_term.insert(block.label, state);
        if !component_states_before_instr.is_empty() {
            analysis
                .block_before_term_components
                .insert(block.label, component_states_before_instr);
        }
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
        resume_case_state_count = u64::try_from(resume_protocol_states.resume_case_states.len())
            .unwrap_or(u64::MAX),
        abrupt_case_block_state_count =
            u64::try_from(resume_protocol_states.abrupt_case_block_states.len())
                .unwrap_or(u64::MAX),
        abrupt_case_route_key_count = u64::try_from(
            resume_protocol_states
                .abrupt_case_keys_by_block
                .values()
                .map(HashSet::len)
                .sum::<usize>(),
        )
        .unwrap_or(u64::MAX),
        dispatch_case_state_count =
            u64::try_from(resume_protocol_states.abrupt_dispatch_case_states.len())
                .unwrap_or(u64::MAX),
        released_case_state_count =
            u64::try_from(resume_protocol_states.released_case_in_states.len()).unwrap_or(u64::MAX),
        ordinary_edge_state_emissions = dataflow_stats.ordinary_edge_state_emissions,
        ordinary_edge_merge_attempts = dataflow_stats.ordinary_edge_merge_attempts,
        ordinary_edge_merge_changes = dataflow_stats.ordinary_edge_merge_changes,
        abrupt_case_pending_pops = dataflow_stats.abrupt_case_pending_pops,
        abrupt_case_edge_state_batches = dataflow_stats.abrupt_case_edge_state_batches,
        abrupt_case_edge_key_emissions = dataflow_stats.abrupt_case_edge_key_emissions,
        abrupt_case_in_merge_attempts = dataflow_stats.abrupt_case_in_merge_attempts,
        abrupt_case_in_merge_changes = dataflow_stats.abrupt_case_in_merge_changes,
        dispatch_case_merge_attempts = dataflow_stats.dispatch_case_merge_attempts,
        dispatch_case_merge_changes = dataflow_stats.dispatch_case_merge_changes,
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
