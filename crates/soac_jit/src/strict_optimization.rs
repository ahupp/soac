//! Source-bound operation requests and per-actual-function runtime witnesses.
//!
//! The checker selects possible sites, never offsets. Class adoption fills an
//! immutable slot array with capabilities obtained from the actual sealed type.
//! A call snapshots that array before binding can run Python. Shared compiled
//! code therefore uses the actual callee's construction identity, including
//! repeated executions of one lexical class, without retaining Python objects.

use std::collections::BTreeMap;
use std::sync::Arc;

use pyo3::prelude::*;
use soac_contracts::{
    AttributeAccess, ClassReference, FieldKind, MethodBinding, ModuleTypeFacts,
    ParticipationProposal, StaticType,
};
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, CallArgPositional, CallableSourceRole, ChildVisitable,
    ConstantExpr, FunctionExecutionMode, FunctionKind, HasMeta, Literal, ModuleShape, ParamKind,
    Visit, VisitMut,
};
use soac_ir_typed::{
    InstrTyped, TypedAttrAccessPlan, TypedBlockPyModuleShape, TypedCall, TypedCallAccessPlan,
    TypedSealedFieldAccessPlan, TypedSealedMethodAccessPlan, TypedSourceBodyTarget,
    TypedSourceCallPlan, ValueFacts,
};

use crate::strict_class_state::{
    SealedFieldCapability, SealedVirtualMethodCapability, StrictClassState,
};
use crate::strict_function::authenticate_strict_function;
use crate::strict_runtime_unavailable;

/// Deterministic indexing shared by the typed planner and actual-function
/// adoption. Source facts only propose a receiver/name to guard; they do not
/// prove that the value has that type, that a field has a checked value type,
/// or that the subclass family is closed. Generated helpers and suspended
/// functions do not acquire a request through module membership or spelling.
pub(crate) fn field_sites<S: ModuleShape>(
    facts: &ModuleTypeFacts,
    function: &BlockPyFunction<S>,
) -> Vec<TypedSealedFieldAccessPlan> {
    let Some(origin) = &function.scope.source_origin else {
        return Vec::new();
    };
    if origin.role != CallableSourceRole::SourceFunction
        || *function.lowered_kind() != FunctionKind::Function
        || origin.definition.module != facts.module
    {
        return Vec::new();
    }
    let mut sites: Vec<_> = facts
        .attribute_sites
        .iter()
        .filter(|site| {
            site.access == AttributeAccess::Read
                && site.identity.enclosing_function == origin.definition
                && site.identity.module == facts.module
                && site.identity.source_digest == facts.source_digest
        })
        .filter_map(|site| {
            let class = match &site.receiver_type {
                StaticType::NominalClass(class) | StaticType::ExactClass(class) => class,
                // For example, ty's synthetic Self can have a known member
                // owner while its relational value type is unsupported. The
                // actual class capability and exact receiver guard, not this
                // member-owner prediction, authorize a storage hit.
                _ => site.declaring_class.as_ref()?,
            };
            // Local declarations distinguish fields from methods precisely.
            // A foreign non-callable member is only an optional request: the
            // actual nominal operand must later publish a sealed-field witness.
            // Foreign callable values use the captured-method path, which
            // falls back normally unless an actual plain method family exists.
            let local = class.definition.module == facts.module;
            if (local
                && (class.source_digest != facts.source_digest
                    || !facts.classes.iter().any(|candidate| {
                        candidate.identity == class.definition
                            && candidate.participation == ParticipationProposal::Candidate
                            && candidate.instance_fields.iter().any(|field| {
                                field.name == site.name
                                    && matches!(
                                        field.field_kind,
                                        FieldKind::InstanceField
                                            | FieldKind::CallableInstanceField
                                            | FieldKind::ShadowableClassDefault
                                    )
                            })
                    })))
                || (!local && matches!(site.value_type, Some(StaticType::Callable(_))))
            {
                return None;
            }
            // OpenWorld, Unknown and other proposal uncertainty cannot make
            // this guarded operation unsound: publication requires the real
            // sealed class/layout, and each load checks the actual receiver,
            // dictionary ownership/aliases and descriptor precedence. No
            // predicted receiver/value type or initialized-field fact flows
            // into the result, including on the generic fallback.
            Some(TypedSealedFieldAccessPlan {
                site: site.identity.clone(),
                receiver_class: class.clone(),
                name: site.name.clone(),
                capability_slot: 0,
            })
        })
        .collect();
    sites.sort_by(|left, right| left.site.cmp(&right.site));
    for (index, site) in sites.iter_mut().enumerate() {
        site.capability_slot = u32::try_from(index).expect("source site count fits source size");
    }
    sites
}

pub(crate) fn method_sites<S: ModuleShape>(
    facts: &ModuleTypeFacts,
    function: &BlockPyFunction<S>,
) -> Vec<TypedSealedMethodAccessPlan> {
    let Some(origin) = &function.scope.source_origin else {
        return Vec::new();
    };
    if origin.role != CallableSourceRole::SourceFunction
        || *function.lowered_kind() != FunctionKind::Function
        || origin.definition.module != facts.module
    {
        return Vec::new();
    }
    let mut sites: Vec<_> = facts
        .attribute_sites
        .iter()
        .filter_map(|site| {
            if site.access != AttributeAccess::Read
                || site.identity.enclosing_function != origin.definition
                || site.identity.module != facts.module
                || site.identity.source_digest != facts.source_digest
            {
                return None;
            }
            let class = match &site.receiver_type {
                StaticType::NominalClass(class) | StaticType::ExactClass(class) => class,
                _ => site.declaring_class.as_ref()?,
            };
            if class.definition.module == facts.module {
                let candidate = facts.classes.iter().find(|candidate| {
                    candidate.identity == class.definition
                        && class.source_digest == facts.source_digest
                        && candidate.participation == ParticipationProposal::Candidate
                })?;
                // A known callable field is a storage operation, never a
                // method-family slot.
                if candidate
                    .instance_fields
                    .iter()
                    .any(|field| field.name == site.name)
                    || !candidate.methods.iter().any(|method| {
                        method.name == site.name
                            && method.binding == MethodBinding::Instance
                            && method.implementation.is_some()
                    })
                {
                    return None;
                }
            } else if !matches!(site.value_type, Some(StaticType::Callable(_))) {
                // Callable is a proposal, not a method-kind or binding proof.
                // Publication and the receiver guard still decide whether this
                // operation can dispatch through a protected method family.
                return None;
            }
            Some(TypedSealedMethodAccessPlan {
                site: site.identity.clone(),
                receiver_class: class.clone(),
                name: site.name.clone(),
                capability_slot: 0,
            })
        })
        .collect();
    sites.sort_by(|left, right| left.site.cmp(&right.site));
    for (index, site) in sites.iter_mut().enumerate() {
        site.capability_slot = u32::try_from(index).expect("source site count fits source size");
    }
    sites
}

fn method_call_matches_site(
    call: &TypedCall<InstrTyped>,
    constants: &[ConstantExpr],
    site: &TypedSealedMethodAccessPlan,
) -> bool {
    let InstrTyped::GetAttrTyped(getter) = call.func.as_ref() else {
        return false;
    };
    let range = getter.meta().range;
    call.frame_namespace.is_none()
        && call.keywords.is_empty()
        && call
            .args
            .iter()
            .all(|argument| matches!(argument, CallArgPositional::Positional(_)))
        && matches!(getter.access, TypedAttrAccessPlan::Generic)
        && constant_attribute_name(getter.attr.as_ref(), constants) == Some(site.name.as_str())
        && range.start().to_u32() == site.site.expression_range.start
        && range.end().to_u32() == site.site.expression_range.end
}

fn validate_method_requests_against_sites(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    constants: &[ConstantExpr],
    sites: &[TypedSealedMethodAccessPlan],
) -> Result<(), String> {
    struct Validator<'a> {
        constants: &'a [ConstantExpr],
        sites: &'a [TypedSealedMethodAccessPlan],
        error: Option<String>,
    }
    impl Visit<InstrTyped> for Validator<'_> {
        fn visit_instr(&mut self, expression: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expression
                && let TypedCallAccessPlan::GuardedSealedMethod(plan) = &call.access
                && (self.sites.get(plan.capability_slot as usize) != Some(plan.as_ref())
                    || !method_call_matches_site(call, self.constants, plan)
                    || call
                        .extra
                        .result_facts()
                        .is_some_and(|facts| facts != ValueFacts::unknown_pyobj()))
            {
                self.error = Some("sealed method call does not match its authenticated source site, binding, or unknown result".into());
            }
            expression.visit_children(self);
        }
    }
    let mut validator = Validator {
        constants,
        sites,
        error: None,
    };
    validator.visit_fn(function);
    validator.error.map_or(Ok(()), Err)
}

fn assign_method_requests(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    constants: &[ConstantExpr],
    sites: &[TypedSealedMethodAccessPlan],
) {
    struct Annotator<'a> {
        constants: &'a [ConstantExpr],
        sites: &'a [TypedSealedMethodAccessPlan],
    }
    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expression: &mut InstrTyped) {
            expression.visit_children_mut(self);
            let InstrTyped::CallTyped(call) = expression else {
                return;
            };
            if !matches!(call.access, TypedCallAccessPlan::Generic)
                || call
                    .extra
                    .result_facts()
                    .is_some_and(|facts| facts != ValueFacts::unknown_pyobj())
            {
                return;
            }
            let Some(site) = self
                .sites
                .iter()
                .find(|site| method_call_matches_site(call, self.constants, site))
            else {
                return;
            };
            call.access = TypedCallAccessPlan::GuardedSealedMethod(Box::new(site.clone()));
            // Overrides and ordinary fallback retain their own ordinary public
            // entries. The caller gets no return-type proof from this slot.
            call.extra.refine_result_facts(ValueFacts::unknown_pyobj());
        }
    }
    Annotator { constants, sites }.visit_fn_mut(function);
}

/// Revalidate after typed rewriting and immediately before mechanical codegen.
/// Slots are meaningful only in this function's authenticated site catalogue.
pub(crate) fn validate_typed_capability_requests(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    verified: Option<&crate::VerifiedStrictModule>,
    constants: &[ConstantExpr],
    source_functions: &[BlockPyFunction<TypedBlockPyModuleShape>],
) -> Result<(), String> {
    let sites = verified.map_or_else(Vec::new, |verified| {
        field_sites(verified.type_facts().facts(), function)
    });
    validate_field_requests_against_sites(function, constants, &sites)?;
    let method_sites = verified.map_or_else(Vec::new, |verified| {
        method_sites(verified.type_facts().facts(), function)
    });
    validate_method_requests_against_sites(function, constants, &method_sites)?;
    let targets = verified.map_or_else(SourceBodyTargets::default, |verified| {
        source_body_targets(verified.type_facts().facts(), source_functions)
    });
    validate_source_call_requests(function, verified, Arc::new(targets))
}

#[derive(Clone, Default)]
struct SourceBodyTargets {
    methods: BTreeMap<(ClassReference, String), TypedSourceBodyTarget>,
    unbound: Vec<(soac_contracts::CallSiteIdentity, TypedSourceBodyTarget)>,
}

/// Match one complete source identity to a unique lowered native body ABI.
/// The actual captured function and its current entry still require runtime
/// authentication; source facts never authorize an unchecked body call.
fn source_body_target(
    source: &soac_contracts::SourceIdentity,
    functions: &[BlockPyFunction<TypedBlockPyModuleShape>],
) -> Option<TypedSourceBodyTarget> {
    let mut matching = functions.iter().filter(|function| {
        function.scope.source_origin.as_ref().is_some_and(|origin| {
            origin.role == CallableSourceRole::SourceFunction && &origin.definition == source
        })
    });
    let function = matching.next()?;
    if matching.next().is_some()
        || *function.lowered_kind() != FunctionKind::Function
        || function.execution_mode() != FunctionExecutionMode::Jit
        || function.params.len() != function.body_params().len()
        || function
            .params
            .iter()
            .any(|parameter| !matches!(parameter.kind, ParamKind::PosOnly | ParamKind::Any))
    {
        return None;
    }
    Some(TypedSourceBodyTarget {
        source: source.clone(),
        function_id: function.function_id,
        argument_count: function.body_params().len(),
    })
}

/// Method capabilities and exact unbound call sites share native-body
/// selection, but retain their distinct source lookup keys. Unbound sites are
/// proposals for the captured callable; current defaults and all required
/// argument binding remain the actual function's binding responsibility.
fn source_body_targets(
    facts: &ModuleTypeFacts,
    functions: &[BlockPyFunction<TypedBlockPyModuleShape>],
) -> SourceBodyTargets {
    let mut targets = SourceBodyTargets::default();
    for class in &facts.classes {
        if class.participation != ParticipationProposal::Candidate {
            continue;
        }
        for method in &class.methods {
            let Some(source) = &method.implementation else {
                continue;
            };
            if method.binding != MethodBinding::Instance || method.generated.is_some() {
                continue;
            }
            let Some(target) = source_body_target(source, functions) else {
                continue;
            };
            targets.methods.insert(
                (
                    ClassReference {
                        definition: class.identity.clone(),
                        source_digest: facts.source_digest,
                    },
                    method.name.clone(),
                ),
                target,
            );
        }
    }
    for site in &facts.call_sites {
        if site.binding != soac_contracts::CallBindingFact::UnboundFunction
            || site.uncertainty != soac_contracts::CallUncertainty::ExactStaticTarget
            || site.identity.module != facts.module
            || site.identity.source_digest != facts.source_digest
            || site.identity.enclosing_function.module != facts.module
        {
            continue;
        }
        let [soac_contracts::CallableTargetFact::SourceFunction(source)] =
            site.candidate_targets.as_slice()
        else {
            continue;
        };
        if source.module != facts.module
            || !facts
                .functions
                .iter()
                .any(|function| &function.identity == source)
        {
            continue;
        }
        if let Some(target) = source_body_target(source, functions) {
            targets.unbound.push((site.identity.clone(), target));
        }
    }
    targets
}

struct SourceCallPlanner {
    caller_source: Option<soac_contracts::SourceIdentity>,
    body_targets: Arc<SourceBodyTargets>,
}

impl SourceCallPlanner {
    fn new(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
        verified: Option<&crate::VerifiedStrictModule>,
        body_targets: Arc<SourceBodyTargets>,
    ) -> Result<Self, String> {
        let caller_source = function
            .scope
            .source_origin
            .as_ref()
            .filter(|origin| origin.role == CallableSourceRole::SourceFunction)
            .map(|origin| origin.definition.clone());
        if let (Some(verified), Some(source)) = (verified, caller_source.as_ref())
            && !verified
                .type_facts()
                .facts()
                .functions
                .iter()
                .any(|fact| &fact.identity == source)
        {
            return Err("source call caller is absent from its authenticated module".into());
        }
        Ok(Self {
            caller_source,
            body_targets,
        })
    }

    fn plan(&self, call: &TypedCall<InstrTyped>) -> Option<TypedSourceCallPlan> {
        let source = self.caller_source.as_ref()?;
        let mut argument_count = match &call.access {
            TypedCallAccessPlan::GuardedSealedMethod(_) => {
                if !matches!(call.func.as_ref(), InstrTyped::GetAttrTyped(_)) {
                    return None;
                }
                1
            }
            TypedCallAccessPlan::Generic if call.keywords.is_empty() => 0,
            _ => return None,
        };
        for argument in &call.args {
            if !matches!(argument, CallArgPositional::Positional(_)) {
                return None;
            }
            argument_count += 1;
        }
        let body_target = match &call.access {
            TypedCallAccessPlan::GuardedSealedMethod(method) => self
                .body_targets
                .methods
                .get(&(method.receiver_class.clone(), method.name.clone()))
                .filter(|target| target.argument_count == argument_count)
                .cloned(),
            TypedCallAccessPlan::Generic => {
                let range = call.meta().range;
                let mut matching = self.body_targets.unbound.iter().filter(|(site, _)| {
                    &site.enclosing_function == source
                        && site.expression_range.start == range.start().to_u32()
                        && site.expression_range.end == range.end().to_u32()
                });
                let (_, target) = matching.next()?;
                if matching.next().is_some() || argument_count > target.argument_count {
                    return None;
                }
                // The ordinary binder reads actual defaults after evaluation.
                argument_count = target.argument_count;
                Some(target.clone())
            }
            _ => return None,
        };
        Some(TypedSourceCallPlan {
            caller_source: source.clone(),
            body_target,
            argument_count,
        })
    }
}

fn validate_source_call_requests(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    verified: Option<&crate::VerifiedStrictModule>,
    body_targets: Arc<SourceBodyTargets>,
) -> Result<(), String> {
    struct Validator {
        planner: SourceCallPlanner,
        valid: bool,
        authenticated: bool,
    }
    impl Visit<InstrTyped> for Validator {
        fn visit_instr(&mut self, expression: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expression
                && let Some(plan) = &call.extra.source_call
                && (!self.authenticated || self.planner.plan(call).as_ref() != Some(plan.as_ref()))
            {
                self.valid = false;
            }
            expression.visit_children(self);
        }
    }
    let mut validator = Validator {
        planner: SourceCallPlanner::new(function, verified, body_targets)?,
        valid: true,
        authenticated: verified.is_some(),
    };
    validator.visit_fn(function);
    if validator.valid {
        Ok(())
    } else {
        Err("source call does not match its authenticated caller and body arity".into())
    }
}

fn assign_source_call_requests(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    verified: &crate::VerifiedStrictModule,
    body_targets: Arc<SourceBodyTargets>,
) -> Result<(), String> {
    let planner = SourceCallPlanner::new(function, Some(verified), body_targets)?;
    apply_source_call_plans(function, planner);
    Ok(())
}

fn apply_source_call_plans(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    planner: SourceCallPlanner,
) {
    // Materialize only source/storage projection data, not a clone of the IR.
    // The visitor writes call sidecars; codegen does no semantic inference.
    struct Annotator(SourceCallPlanner);
    impl VisitMut<InstrTyped> for Annotator {
        fn visit_instr_mut(&mut self, expression: &mut InstrTyped) {
            expression.visit_children_mut(self);
            if let InstrTyped::CallTyped(call) = expression {
                call.extra.source_call = self.0.plan(call).map(Box::new);
            }
        }
    }
    Annotator(planner).visit_fn_mut(function);
}

fn constant_attribute_name<'a>(
    expression: &InstrTyped,
    constants: &'a [ConstantExpr],
) -> Option<&'a str> {
    if let InstrTyped::Load(load) = expression
        && let Some(index) = load.name.location.as_constant()
        && let Some(ConstantExpr::Literal(value)) = constants.get(index as usize)
        && let Literal::StringLiteral(value) = value.as_literal()
    {
        Some(value.value.as_str())
    } else {
        None
    }
}

fn validate_field_requests_against_sites(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    constants: &[ConstantExpr],
    sites: &[TypedSealedFieldAccessPlan],
) -> Result<(), String> {
    struct Validator<'a> {
        sites: &'a [TypedSealedFieldAccessPlan],
        constants: &'a [ConstantExpr],
        error: Option<String>,
    }
    impl Visit<InstrTyped> for Validator<'_> {
        fn visit_instr(&mut self, expression: &InstrTyped) {
            if self.error.is_some() {
                return;
            }
            match expression {
                InstrTyped::GetAttrTyped(getter) => {
                    if let TypedAttrAccessPlan::GuardedSealedField(plan) = &getter.access {
                        let name = constant_attribute_name(getter.attr.as_ref(), self.constants);
                        let range = getter.meta().range;
                        if self.sites.get(plan.capability_slot as usize) != Some(plan.as_ref())
                            || name != Some(plan.name.as_str())
                            || plan.site.expression_range.start != range.start().to_u32()
                            || plan.site.expression_range.end != range.end().to_u32()
                        {
                            self.error = Some("sealed field plan does not match its authenticated function/site/slot".to_owned());
                        }
                    }
                }
                InstrTyped::SetAttrTyped(setter)
                    if matches!(setter.access, TypedAttrAccessPlan::GuardedSealedField(_)) =>
                {
                    self.error =
                        Some("sealed field read capability cannot authorize a store".to_owned());
                }
                _ => {}
            }
            expression.visit_children(self);
        }
    }
    let mut validator = Validator {
        sites,
        constants,
        error: None,
    };
    validator.visit_fn(function);
    validator.error.map_or(Ok(()), Err)
}

fn assign_field_requests(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    constants: &[ConstantExpr],
    sites: &[TypedSealedFieldAccessPlan],
) {
    struct Annotator<'a> {
        sites: &'a [TypedSealedFieldAccessPlan],
        constants: &'a [ConstantExpr],
    }
    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expression: &mut InstrTyped) {
            expression.visit_children_mut(self);
            let InstrTyped::GetAttrTyped(getter) = expression else {
                return;
            };
            // Do not weaken an already planned value/guard after dependent
            // rewrites. The first consumer changes only a generic lookup with
            // no result-type information; the source slot catalogue remains
            // deterministic even when an IR site is not selected.
            if !matches!(getter.access, TypedAttrAccessPlan::Generic)
                || getter
                    .extra
                    .result_facts()
                    .is_some_and(|facts| facts != ValueFacts::unknown_pyobj())
            {
                return;
            }
            let range = getter.meta().range;
            let Some(name) = constant_attribute_name(getter.attr.as_ref(), self.constants) else {
                return;
            };
            let Some(site) = self.sites.iter().find(|site| {
                site.name == name
                    && site.site.expression_range.start == range.start().to_u32()
                    && site.site.expression_range.end == range.end().to_u32()
            }) else {
                return;
            };
            getter.access = TypedAttrAccessPlan::GuardedSealedField(Box::new(site.clone()));
            // An indexed storage witness is never a checked-value proof.
            // Set unknown explicitly so the shared fact store is also updated.
            getter
                .extra
                .refine_result_facts(ValueFacts::unknown_pyobj());
        }
    }
    Annotator { sites, constants }.visit_fn_mut(function);
}

pub(crate) fn annotate_typed_capability_requests(
    module: &mut BlockPyModule<TypedBlockPyModuleShape>,
    verified: &crate::VerifiedStrictModule,
) -> Result<(), String> {
    let targets = Arc::new(source_body_targets(
        verified.type_facts().facts(),
        &module.callable_defs,
    ));
    for function in &mut module.callable_defs {
        let sites = field_sites(verified.type_facts().facts(), function);
        assign_field_requests(function, &module.module_constants, &sites);
        let methods = method_sites(verified.type_facts().facts(), function);
        assign_method_requests(function, &module.module_constants, &methods);
        assign_source_call_requests(function, verified, Arc::clone(&targets))?;
        validate_field_requests_against_sites(function, &module.module_constants, &sites)?;
        validate_method_requests_against_sites(function, &module.module_constants, &methods)?;
        validate_source_call_requests(function, Some(verified), Arc::clone(&targets))?;
    }
    Ok(())
}

/// Select source-owned call regions before expression linearization so
/// lookup, arguments, and both captured-call continuations stay together.
pub(crate) fn annotate_typed_call_requests(
    module: &mut BlockPyModule<TypedBlockPyModuleShape>,
    verified: &crate::VerifiedStrictModule,
) -> Result<(), String> {
    let targets = Arc::new(source_body_targets(
        verified.type_facts().facts(),
        &module.callable_defs,
    ));
    for function in &mut module.callable_defs {
        let sites = method_sites(verified.type_facts().facts(), function);
        assign_method_requests(function, &module.module_constants, &sites);
        validate_method_requests_against_sites(function, &module.module_constants, &sites)?;
        assign_source_call_requests(function, verified, Arc::clone(&targets))?;
        validate_source_call_requests(function, Some(verified), Arc::clone(&targets))?;
    }
    Ok(())
}

pub(crate) fn sealed_method_site_count(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> usize {
    struct Counter(usize);
    impl Visit<InstrTyped> for Counter {
        fn visit_instr(&mut self, expression: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expression
                && matches!(call.access, TypedCallAccessPlan::GuardedSealedMethod(_))
            {
                self.0 += 1;
            }
            expression.visit_children(self);
        }
    }
    let mut counter = Counter(0);
    counter.visit_fn(function);
    counter.0
}

pub(crate) fn checked_fixed_body_site_count(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> usize {
    struct Counter(usize);
    impl Visit<InstrTyped> for Counter {
        fn visit_instr(&mut self, expression: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expression
                && call
                    .extra
                    .source_call
                    .as_ref()
                    .is_some_and(|plan| plan.body_target.is_some())
            {
                self.0 += 1;
            }
            expression.visit_children(self);
        }
    }
    let mut counter = Counter(0);
    counter.visit_fn(function);
    counter.0
}

pub(crate) fn sealed_field_site_count(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> usize {
    struct Counter(usize);
    impl Visit<InstrTyped> for Counter {
        fn visit_instr(&mut self, expression: &InstrTyped) {
            if let InstrTyped::GetAttrTyped(getter) = expression
                && matches!(getter.access, TypedAttrAccessPlan::GuardedSealedField(_))
            {
                self.0 += 1;
            }
            expression.visit_children(self);
        }
    }
    let mut counter = Counter(0);
    counter.visit_fn(function);
    counter.0
}

pub(crate) struct StrictCapabilitySlots<T> {
    // Addresses are only dereferenced while this array owns the corresponding
    // Arc, and after the active function's source-site plan selected this slot.
    slots: Box<[usize]>,
    witnesses: Box<[Option<Arc<T>>]>,
}

pub(crate) type StrictFieldCapabilities = StrictCapabilitySlots<SealedFieldCapability>;
pub(crate) type StrictMethodCapabilities = StrictCapabilitySlots<SealedVirtualMethodCapability>;

impl<T> StrictCapabilitySlots<T> {
    fn new(witnesses: Vec<Option<Arc<T>>>) -> Self {
        let slots = witnesses
            .iter()
            .map(|witness| {
                witness
                    .as_ref()
                    .map_or(0, |value| Arc::as_ptr(value) as usize)
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots,
            witnesses: witnesses.into_boxed_slice(),
        }
    }

    pub(crate) fn raw_slots(&self) -> (*const usize, usize) {
        (self.slots.as_ptr(), self.slots.len())
    }

    /// Publish only previously absent witnesses. Class capability creation may
    /// allocate, so another cold publication can have filled slots since the
    /// caller took its snapshot. Never replace those witnesses or their owners.
    fn extend(
        previous: Option<&Arc<Self>>,
        proposed: Arc<Self>,
    ) -> Result<Arc<Self>, &'static str> {
        let Some(previous) = previous else {
            return Ok(proposed);
        };
        if previous.witnesses.len() != proposed.witnesses.len() {
            return Err("capability source site layout changed during publication");
        }
        let mut witnesses = previous.witnesses.to_vec();
        let mut changed = false;
        for (current, added) in witnesses.iter_mut().zip(&proposed.witnesses) {
            if current.is_none() && added.is_some() {
                *current = added.clone();
                changed = true;
            }
        }
        Ok(if changed {
            Arc::new(Self::new(witnesses))
        } else {
            Arc::clone(previous)
        })
    }
}

/// Mandatory metadata may be sealed before its module's bindings. That is
/// not permission to publish a captured checked-entry or layout capability.
/// This guard adds no new capability; it withholds the existing optional paths
/// until the same execution has finished all required module-target binding.
fn optional_function_bindings_ready(
    py: Python<'_>,
    auth: &crate::strict_function::AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<bool> {
    if auth.is_interpreter() || !auth.is_finalized() || auth.awaits_module_nominals() {
        return Ok(false);
    }
    let globals = auth.globals()?;
    auth.execution_ref()
        .bindings_are_final(py, &globals, auth.verified_module())
}

/// The caller supplies either the actual declaring namespace's class or an
/// authenticated nominal operand. Source/profile lookup alone cannot reach
/// this publication path.
fn bind_function_field_capabilities(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    class: &StrictClassState<'_>,
) -> PyResult<()> {
    if !class.is_finalized() {
        return Err(strict_runtime_unavailable(
            py,
            "field capability publication requires class seal",
        ));
    }
    let Some(auth) = authenticate_strict_function(py, function)? else {
        return Ok(());
    };
    if auth.is_interpreter() {
        return Ok(()); // No compiler target/layout from a native interpreter owner.
    }
    if !optional_function_bindings_ready(py, &auth)?
        || auth
            .origin()
            .is_none_or(|origin| origin.role != CallableSourceRole::SourceFunction)
    {
        return Ok(());
    }
    let shared = Arc::clone(auth.module_state()?);
    let template = shared
        .lookup_function_template(auth.function_id()?)
        .map_err(|error| strict_runtime_unavailable(py, error))?
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "field capability function has no template")
        })?;
    let sites = field_sites(
        auth.verified_module().type_facts().facts(),
        template.function(),
    );
    if sites.is_empty() {
        return Ok(());
    }
    // Only clone immutable Rust data while borrowing mutable native metadata.
    // Obtaining a class capability below may allocate; keep no metadata borrow
    // across that boundary, even for a currently finalized function.
    let previous = unsafe {
        let metadata =
            crate::py_function_jit_extra(function.as_ptr()).map_err(|()| PyErr::fetch(py))?;
        (*metadata).function_env.strict_field_capabilities.clone()
    };
    if previous
        .as_ref()
        .is_some_and(|previous| previous.witnesses.len() != sites.len())
    {
        return Err(strict_runtime_unavailable(
            py,
            "field capability site layout changed",
        ));
    }
    let mut witnesses = previous.as_ref().map_or_else(
        || vec![None; sites.len()],
        |previous| previous.witnesses.to_vec(),
    );
    let class_digest = class.verified_module().type_facts().facts().source_digest;
    let mut changed = false;
    for site in &sites {
        if site.receiver_class.definition != *class.source()
            || site.receiver_class.source_digest != class_digest
        {
            continue;
        }
        let slot = &mut witnesses[site.capability_slot as usize];
        if slot.is_none()
            && let Some(capability) = class.sealed_field(&site.name)?
        {
            debug_assert_eq!(capability.source(), class.source());
            debug_assert_eq!(capability.field_name(), site.name);
            *slot = Some(Arc::new(capability));
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    let native_object_slot_count = witnesses
        .iter()
        .flatten()
        .filter(|capability| {
            capability.storage_kind()
                == crate::strict_class_state::SealedFieldStorageKind::NativeObjectMember
        })
        .count();
    let indexed_dictionary_slot_count =
        witnesses.iter().flatten().count() - native_object_slot_count;
    let capabilities = Arc::new(StrictFieldCapabilities::new(witnesses));
    // Re-authenticate after allocations. A terminal native owner is an error,
    // never permission to retain or revive its optional optimization metadata.
    let current = authenticate_strict_function(py, function)?.ok_or_else(|| {
        strict_runtime_unavailable(py, "field capability target lost its source owner")
    })?;
    if current.owner().as_ptr() != auth.owner().as_ptr() {
        return Err(strict_runtime_unavailable(
            py,
            "field capability target owner changed",
        ));
    }
    unsafe {
        let metadata =
            crate::py_function_jit_extra(function.as_ptr()).map_err(|()| PyErr::fetch(py))?;
        let capabilities = StrictFieldCapabilities::extend(
            (*metadata).function_env.strict_field_capabilities.as_ref(),
            capabilities,
        )
        .map_err(|error| strict_runtime_unavailable(py, error))?;
        (*metadata)
            .function_env
            .set_strict_field_capabilities(Some(capabilities));
    }
    tracing::info!(
        target: "soac_jit_codegen",
        event = "soac.strict_field_capabilities",
        module_name = auth.verified_module().type_facts().facts().module.module_name,
        function_id = auth.function_id()?.to_string(),
        function_qualname = template.function().names.qualname,
        class_qualname = class.source().lexical_qualname,
        slot_count = sites.len(),
        indexed_dictionary_slot_count,
        native_object_slot_count,
        "strict_field_capabilities",
    );
    Ok(())
}

fn bind_function_method_capabilities(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    class: &StrictClassState<'_>,
) -> PyResult<()> {
    let Some(auth) = authenticate_strict_function(py, function)? else {
        return Ok(());
    };
    if auth.is_interpreter() {
        return Ok(()); // No compiler target/layout from a native interpreter owner.
    }
    if !class.is_finalized()
        || !optional_function_bindings_ready(py, &auth)?
        || auth
            .origin()
            .is_none_or(|origin| origin.role != CallableSourceRole::SourceFunction)
    {
        return Ok(());
    }
    let template = auth
        .module_state()?
        .lookup_function_template(auth.function_id()?)
        .map_err(|error| strict_runtime_unavailable(py, error))?
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "method capability function has no template")
        })?;
    let sites = method_sites(
        auth.verified_module().type_facts().facts(),
        template.function(),
    );
    if sites.is_empty() {
        return Ok(());
    }
    // Different authenticated nominal operands may contribute disjoint slots.
    // Existing slots never change their actual construction identity.
    let previous = unsafe {
        let metadata =
            crate::py_function_jit_extra(function.as_ptr()).map_err(|()| PyErr::fetch(py))?;
        (*metadata).function_env.strict_method_capabilities.clone()
    };
    if previous
        .as_ref()
        .is_some_and(|previous| previous.witnesses.len() != sites.len())
    {
        return Err(strict_runtime_unavailable(
            py,
            "method capability source site layout changed",
        ));
    }
    let mut witnesses = previous.as_ref().map_or_else(
        || vec![None; sites.len()],
        |previous| previous.witnesses.to_vec(),
    );
    let mut changed = false;
    for site in &sites {
        let slot = &mut witnesses[site.capability_slot as usize];
        if slot.is_none()
            && let Some(capability) =
                class.sealed_virtual_method_for_source(&site.receiver_class, &site.name)?
        {
            *slot = Some(Arc::new(capability));
            changed = true;
        }
    }
    let available = witnesses.iter().filter(|witness| witness.is_some()).count();
    if !changed {
        return Ok(());
    }
    let capabilities = Arc::new(StrictMethodCapabilities::new(witnesses));
    // Revalidate after allocating and release all temporary native views before
    // touching metadata. The actual function remains pinned by this adoption.
    let current = authenticate_strict_function(py, function)?.ok_or_else(|| {
        strict_runtime_unavailable(py, "method capability function lost its source owner")
    })?;
    if current.owner().as_ptr() != auth.owner().as_ptr() {
        return Err(strict_runtime_unavailable(
            py,
            "method capability function owner changed",
        ));
    }
    unsafe {
        let metadata =
            crate::py_function_jit_extra(function.as_ptr()).map_err(|()| PyErr::fetch(py))?;
        let capabilities = StrictMethodCapabilities::extend(
            (*metadata).function_env.strict_method_capabilities.as_ref(),
            capabilities,
        )
        .map_err(|error| strict_runtime_unavailable(py, error))?;
        (*metadata)
            .function_env
            .set_strict_method_capabilities(Some(capabilities));
    }
    tracing::info!(
        target: "soac_jit_codegen",
        event = "soac.strict_method_capabilities",
        module_name = auth.verified_module().type_facts().facts().module.module_name,
        function_id = auth.function_id()?.to_string(),
        function_qualname = template.function().names.qualname,
        class_qualname = class.source().lexical_qualname,
        slot_count = sites.len(),
        available_slot_count = available,
        "strict_method_capabilities",
    );
    Ok(())
}

pub(crate) fn bind_owned_function_capabilities(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
    class: &StrictClassState<'_>,
) -> PyResult<()> {
    let Some(auth) = authenticate_strict_function(py, function)? else {
        return Ok(());
    };
    if auth.is_interpreter() {
        return Ok(()); // No compiler target/layout from a native interpreter owner.
    }
    // Inherited functions retain their original declaring environment. A
    // later execution of an equal source definition cannot claim this owner.
    if !class.is_finalized()
        || !optional_function_bindings_ready(py, &auth)?
        || auth
            .creation_execution()
            .is_none_or(|execution| !Arc::ptr_eq(execution, class.namespace_execution()))
    {
        return Ok(());
    }
    bind_function_field_capabilities(py, function, class)?;
    bind_function_method_capabilities(py, function, class)?;
    bind_nominal_function_capabilities(py, function)
}

/// Cold publication for free functions and foreign-class parameters. The
/// nominal resolver has already captured these exact actual types from the
/// function's authenticated globals or provider cells. Nominal acceptance is
/// not a layout proof: each operand must independently have a permanent native
/// class seal before it can contribute an optional guarded capability.
pub(crate) fn bind_nominal_function_capabilities(
    py: Python<'_>,
    function: &Bound<'_, PyAny>,
) -> PyResult<()> {
    let Some(auth) = authenticate_strict_function(py, function)? else {
        return Ok(());
    };
    if auth.is_interpreter() {
        return Ok(()); // No compiler target/layout from a native interpreter owner.
    }
    if !optional_function_bindings_ready(py, &auth)? {
        return Ok(());
    }
    let mut targets = BTreeMap::new();
    for binding in auth.capability_nominal_bindings() {
        let Some(actual) = auth.bound_nominal_target(binding)? else {
            continue;
        };
        match targets.entry(binding.class.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Some(actual));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if entry
                    .get()
                    .as_ref()
                    .is_some_and(|previous| previous.as_ptr() != actual.as_ptr())
                {
                    // Equal source identities can denote distinct factory
                    // classes. No arbitrary "latest class" resolves a site.
                    entry.insert(None);
                }
            }
        }
    }
    for (reference, actual) in targets {
        let Some(actual) = actual else {
            continue;
        };
        let Some(class) = crate::strict_class_state::for_actual_type(py, &actual)? else {
            continue;
        };
        if !class.is_finalized()
            || *class.source() != reference.definition
            || class.verified_module().type_facts().facts().source_digest != reference.source_digest
        {
            continue;
        }
        bind_function_field_capabilities(py, function, &class)?;
        bind_function_method_capabilities(py, function, &class)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_contracts::{
        AnnotationOrigin, AttributeSiteFact, AttributeSiteIdentity, CallableSignature,
        ClassDictionarySemantics, ClassOpenness, ClassReference, ClassTypeFact, DefaultFact,
        DefinitionKind, DescriptorFact, FieldKind, FieldReadPolicy, FieldTypeFact,
        FieldWritePolicy, InheritanceFact, InitializationPolicy, MetaclassFact, MethodTypeFact,
        OverridePolicy, ParticipationProposal, ResolvedStrictPolicy, SourceDialect, SourceIdentity,
        SourceRange, UncertaintyReason,
    };
    use soac_core::block_py::{CallableSourceOrigin, HasSemanticInstrId, InstrId, InstrKey};
    use soac_ir_typed::{FactStore, PyExactType, PyObjFacts, lower_blockpy_module_to_typed};
    use std::collections::BTreeSet;

    const SOURCE: &str = "class Box:\n    first = 1\n    second = 2\n    def read(self):\n        return self.second, self.first\n";

    #[test]
    fn capability_publication_only_fills_absent_slots_and_preserves_active_snapshots() {
        let original_owner = Arc::new(1);
        let added_owner = Arc::new(2);
        let original = Arc::new(StrictCapabilitySlots::new(vec![
            Some(Arc::clone(&original_owner)),
            None,
        ]));
        let proposed = Arc::new(StrictCapabilitySlots::new(vec![
            Some(Arc::new(99)),
            Some(Arc::clone(&added_owner)),
        ]));
        let merged = StrictCapabilitySlots::extend(Some(&original), proposed).unwrap();
        assert_eq!(
            original.slots[1], 0,
            "active calls retain their immutable snapshot"
        );
        assert_eq!(merged.slots[0], Arc::as_ptr(&original_owner) as usize);
        assert_eq!(merged.slots[1], Arc::as_ptr(&added_owner) as usize);
        assert!(Arc::ptr_eq(
            &merged,
            &StrictCapabilitySlots::extend(Some(&merged), Arc::clone(&original)).unwrap()
        ));
        assert!(
            StrictCapabilitySlots::extend(
                Some(&merged),
                Arc::new(StrictCapabilitySlots::new(vec![None]))
            )
            .is_err()
        );
    }

    /// A structured planning fixture, not an authenticated runtime fixture.
    /// The real lowerer supplies operation ranges and semantic instruction IDs;
    /// the small logical proposal only selects optional capability requests.
    fn fixture() -> (
        ModuleTypeFacts,
        BlockPyModule<TypedBlockPyModuleShape>,
        usize,
    ) {
        fixture_source(SOURCE)
    }

    fn fixture_source(
        source: &str,
    ) -> (
        ModuleTypeFacts,
        BlockPyModule<TypedBlockPyModuleShape>,
        usize,
    ) {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(source)
            .expect("field request source lowers")
            .blockpy_module;
        let mut module = lower_blockpy_module_to_typed(lowered);
        let index = module
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "Box.read")
            .expect("actual source method");
        let mut facts = ModuleTypeFacts::new(
            "field_request_fixture",
            source.as_bytes(),
            SourceDialect::SoacStrict,
            ResolvedStrictPolicy {
                strict_assign: true,
                checked_attr: true,
                ..Default::default()
            },
        )
        .unwrap();
        let class = ClassReference {
            definition: SourceIdentity {
                module: facts.module.clone(),
                lexical_qualname: "Box".into(),
                source_range: SourceRange::new(0, facts.source_size),
                definition_kind: DefinitionKind::Class,
            },
            source_digest: facts.source_digest,
        };
        let method = SourceIdentity {
            module: facts.module.clone(),
            lexical_qualname: "Box.read".into(),
            source_range: SourceRange::new(
                source.find("def read").unwrap() as u32,
                facts.source_size,
            ),
            definition_kind: DefinitionKind::Function,
        };
        module.callable_defs[index].scope.source_origin = Some(CallableSourceOrigin {
            definition: method.clone(),
            role: CallableSourceRole::SourceFunction,
        });
        facts.classes.push(ClassTypeFact {
            identity: class.definition.clone(),
            bases: Vec::new(),
            metaclass: MetaclassFact::BuiltinType,
            decorators: Vec::new(),
            participation: ParticipationProposal::Candidate,
            dictionary: ClassDictionarySemantics::DictionaryBearing,
            instance_fields: ["first", "second"]
                .into_iter()
                .map(|name| FieldTypeFact {
                    name: name.into(),
                    declaring_class: class.clone(),
                    value_type: StaticType::Any,
                    annotation_origin: AnnotationOrigin::Inferred,
                    annotation_definition: None,
                    field_kind: FieldKind::ShadowableClassDefault,
                    read_policy: FieldReadPolicy::InstanceThenClassDefault,
                    write_policy: FieldWritePolicy::DeclaredField,
                    initialization: InitializationPolicy::MayBeAbsent,
                    default: DefaultFact::Unknown,
                    descriptor: DescriptorFact::default(),
                    uncertainty: BTreeSet::from([UncertaintyReason::Unknown]),
                })
                .collect(),
            methods: Vec::new(),
            class_members: Vec::new(),
            inheritance: InheritanceFact {
                linearized_bases: Vec::new(),
                complete: true,
            },
            openness: ClassOpenness::OpenSubclassFamily,
            transform: None,
            uncertainty: BTreeSet::from([UncertaintyReason::OpenWorld]),
        });
        // Deliberately reverse the source order: slots are source-site ordered,
        // not checker traversal order or a predicted physical field offset.
        for name in ["first", "second"] {
            let expression = format!("self.{name}");
            let start = source.find(&expression).unwrap() as u32;
            facts.attribute_sites.push(AttributeSiteFact {
                identity: AttributeSiteIdentity {
                    module: facts.module.clone(),
                    source_digest: facts.source_digest,
                    enclosing_function: method.clone(),
                    expression_range: SourceRange::new(start, start + expression.len() as u32),
                },
                name: name.into(),
                access: AttributeAccess::Read,
                receiver_type: StaticType::NominalClass(class.clone()),
                value_type: Some(StaticType::Any),
                declaring_class: Some(class.clone()),
                uncertainty: BTreeSet::from([UncertaintyReason::OpenWorld]),
            });
        }
        (facts, module, index)
    }

    /// The same logical exact-source proposal as the genuine defaults fixture,
    /// attached to real lowered operations. This does not create verification,
    /// install a runtime owner, or authorize a call.
    fn unbound_default_fixture() -> (
        ModuleTypeFacts,
        BlockPyModule<TypedBlockPyModuleShape>,
        usize,
    ) {
        unbound_default_fixture_with_arguments("value")
    }

    fn unbound_default_fixture_with_arguments(
        arguments: &str,
    ) -> (
        ModuleTypeFacts,
        BlockPyModule<TypedBlockPyModuleShape>,
        usize,
    ) {
        let source = format!(
            "def callee(value, increment=5):\n    return value + increment\n\ndef run(value):\n    return callee({arguments})\n"
        );
        let mut module = lower_blockpy_module_to_typed(
            soac_lowering::lower_python_to_blockpy_for_testing(&source)
                .expect("unbound default source lowers")
                .blockpy_module,
        );
        let mut facts = ModuleTypeFacts::new(
            "checked_unbound_fixture",
            source.as_bytes(),
            SourceDialect::SoacStrict,
            ResolvedStrictPolicy {
                strict_assign: true,
                ..Default::default()
            },
        )
        .unwrap();
        let split = source.find("def run").unwrap();
        for (name, start, end, parameters) in [
            (
                "callee",
                0,
                source[..split].trim_end().len(),
                vec!["value", "increment"],
            ),
            ("run", split, source.trim_end().len(), vec!["value"]),
        ] {
            let identity = SourceIdentity {
                module: facts.module.clone(),
                lexical_qualname: name.into(),
                source_range: SourceRange::new(start as u32, end as u32),
                definition_kind: DefinitionKind::Function,
            };
            let function = module
                .callable_defs
                .iter_mut()
                .find(|function| function.names.qualname == name)
                .unwrap();
            function.scope.source_origin = Some(CallableSourceOrigin {
                definition: identity.clone(),
                role: CallableSourceRole::SourceFunction,
            });
            facts.functions.push(soac_contracts::FunctionTypeFact {
                identity,
                function_kind: soac_contracts::FunctionKind::Synchronous,
                signature: CallableSignature {
                    parameters: parameters
                        .into_iter()
                        .map(|name| soac_contracts::ParameterTypeFact {
                            name: name.into(),
                            kind: soac_contracts::ParameterKind::PositionalOrKeyword,
                            value_type: StaticType::Unknown,
                            annotation_origin: AnnotationOrigin::Inferred,
                            default: if name == "increment" {
                                DefaultFact::Value {
                                    value_type: Box::new(StaticType::Unknown),
                                    literal: None,
                                }
                            } else {
                                DefaultFact::Missing
                            },
                        })
                        .collect(),
                    return_type: StaticType::Unknown,
                    return_annotation_origin: AnnotationOrigin::Absent,
                    uncertainty: BTreeSet::from([UncertaintyReason::Unknown]),
                },
                decorators: Vec::new(),
                uncertainty: BTreeSet::from([UncertaintyReason::Unknown]),
            });
        }
        let expression = format!("callee({arguments})");
        let call_start = source.rfind(&expression).unwrap();
        facts.call_sites.push(soac_contracts::CallSiteFact {
            identity: soac_contracts::CallSiteIdentity {
                module: facts.module.clone(),
                source_digest: facts.source_digest,
                enclosing_function: facts.functions[1].identity.clone(),
                expression_range: SourceRange::new(
                    call_start as u32,
                    (call_start + expression.len()) as u32,
                ),
                expression_kind: soac_contracts::CallExpressionKind::Call,
            },
            receiver: None,
            attribute_name: None,
            candidate_targets: vec![soac_contracts::CallableTargetFact::SourceFunction(
                facts.functions[0].identity.clone(),
            )],
            binding: soac_contracts::CallBindingFact::UnboundFunction,
            signature: facts.functions[0].signature.clone(),
            result_type: StaticType::Unknown,
            uncertainty: soac_contracts::CallUncertainty::ExactStaticTarget,
        });
        let index = module
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "run")
            .unwrap();
        (facts, module, index)
    }

    #[test]
    fn checked_unbound_default_call_selects_a_full_bound_native_body() {
        let (facts, mut module, index) = unbound_default_fixture();
        let targets = Arc::new(source_body_targets(&facts, &module.callable_defs));
        let planner = SourceCallPlanner::new(&module.callable_defs[index], None, targets).unwrap();
        apply_source_call_plans(&mut module.callable_defs[index], planner);
        assert_eq!(
            checked_fixed_body_site_count(&module.callable_defs[index]),
            1,
            "an exact unbound source call with an omitted default needs a checked body plan"
        );
    }

    fn source_calls(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
    ) -> Vec<TypedCall<InstrTyped>> {
        struct Calls(Vec<TypedCall<InstrTyped>>);
        impl Visit<InstrTyped> for Calls {
            fn visit_instr(&mut self, instruction: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = instruction
                    && call.extra.source_call.is_some()
                {
                    self.0.push(call.clone());
                }
                instruction.visit_children(self);
            }
        }
        let mut calls = Calls(Vec::new());
        calls.visit_fn(function);
        calls.0
    }

    #[test]
    fn source_unbound_defaults_bind_full_arity_without_promoting_values() {
        let (facts, mut module, index) = unbound_default_fixture();
        let targets = Arc::new(source_body_targets(&facts, &module.callable_defs));
        let function = &mut module.callable_defs[index];
        let planner = SourceCallPlanner::new(function, None, Arc::clone(&targets)).unwrap();
        apply_source_call_plans(function, planner);
        let calls = source_calls(function);
        assert_eq!(calls.len(), 1);
        let plan = calls[0].extra.source_call.as_ref().unwrap();
        assert_eq!(plan.argument_count, 2);
        let target = plan.body_target.as_ref().unwrap();
        assert_eq!(target.source, facts.functions[0].identity);
        assert_eq!(target.argument_count, 2);
        assert_eq!(calls[0].args.len(), 1);
        assert!(
            calls[0]
                .extra
                .result_facts()
                .is_none_or(|fact| fact == ValueFacts::unknown_pyobj())
        );
        assert!(
            validate_source_call_requests(function, None, targets).is_err(),
            "logical source/projection data cannot authorize a checked body"
        );
    }

    #[test]
    fn checked_unbound_call_requires_its_exact_unique_signed_source_site() {
        use soac_contracts::{CallBindingFact, CallUncertainty, CallableTargetFact};
        let (facts, module, index) = unbound_default_fixture();
        for variant in 0..11 {
            let mut changed = facts.clone();
            let site = &mut changed.call_sites[0];
            match variant {
                0 => site.binding = CallBindingFact::Dynamic,
                1 => site.uncertainty = CallUncertainty::Dynamic,
                2 => site.candidate_targets.clear(),
                3 => site.candidate_targets.push(CallableTargetFact::Dynamic),
                4 => {
                    site.identity.module = ModuleTypeFacts::new(
                        "other",
                        b"other",
                        SourceDialect::SoacStrict,
                        ResolvedStrictPolicy {
                            strict_assign: true,
                            ..Default::default()
                        },
                    )
                    .unwrap()
                    .module
                }
                5 => site.identity.source_digest = soac_contracts::Fingerprint::digest(b"other"),
                6 => site.identity.enclosing_function.lexical_qualname = "other".into(),
                7 => site.identity.expression_range.end -= 1,
                8 => changed.functions.clear(),
                9 => {
                    let CallableTargetFact::SourceFunction(source) = &mut site.candidate_targets[0]
                    else {
                        unreachable!()
                    };
                    source.source_range.end -= 1;
                }
                _ => changed.call_sites.push(changed.call_sites[0].clone()),
            }
            let mut function = module.callable_defs[index].clone();
            let targets = Arc::new(source_body_targets(&changed, &module.callable_defs));
            let planner = SourceCallPlanner::new(&function, None, targets).unwrap();
            apply_source_call_plans(&mut function, planner);
            assert!(source_calls(&function).is_empty(), "site variant {variant}");
        }
        let mut changed = module.clone();
        let callee = changed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "callee")
            .unwrap()
            .clone();
        changed.callable_defs.push(callee);
        assert!(
            source_body_targets(&facts, &changed.callable_defs)
                .unbound
                .is_empty(),
            "a duplicate native source body is not a fixed target"
        );
    }

    #[test]
    fn checked_unbound_call_keeps_callbacks_inside_the_captured_call_region() {
        let (facts, mut module, index) = unbound_default_fixture_with_arguments("value()");
        let targets = Arc::new(source_body_targets(&facts, &module.callable_defs));
        let function = &mut module.callable_defs[index];
        let planner = SourceCallPlanner::new(function, None, targets).unwrap();
        apply_source_call_plans(function, planner);
        let before = source_calls(function);
        assert_eq!(before.len(), 1);
        assert!(matches!(before[0].func.as_ref(), InstrTyped::Load(_)));
        assert!(matches!(
            before[0].args.as_slice(),
            [CallArgPositional::Positional(InstrTyped::CallTyped(_))]
        ));
        assert_eq!(
            before[0].extra.source_call.as_ref().unwrap().argument_count,
            2
        );
        soac_opt::passes::linearize_typed_function_expressions(function).unwrap();
        let after = source_calls(function);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].extra.source_call, before[0].extra.source_call);
        assert!(matches!(after[0].func.as_ref(), InstrTyped::Load(_)));
        assert!(matches!(
            after[0].args.as_slice(),
            [CallArgPositional::Positional(InstrTyped::CallTyped(_))]
        ));
    }

    #[test]
    fn checked_unbound_keyword_unpack_and_oversized_calls_keep_public_binding() {
        for arguments in ["value, increment=5", "*value", "value, value, value"] {
            let (facts, mut module, index) = unbound_default_fixture_with_arguments(arguments);
            let targets = Arc::new(source_body_targets(&facts, &module.callable_defs));
            let function = &mut module.callable_defs[index];
            let planner = SourceCallPlanner::new(function, None, targets).unwrap();
            apply_source_call_plans(function, planner);
            assert!(source_calls(function).is_empty(), "arguments: {arguments}");
        }
    }

    fn method_fixture() -> (
        ModuleTypeFacts,
        BlockPyModule<TypedBlockPyModuleShape>,
        usize,
    ) {
        let source = "class Box:\n    def first(self, value):\n        return value\n    def second(self, value):\n        return value\n    def read(self, value, argument):\n        return self.second(argument()), self.first(value)\n";
        let (mut facts, mut module, index) = fixture_source(source);
        let class = facts.attribute_sites[0].declaring_class.clone().unwrap();
        facts.classes[0].instance_fields.clear();
        for (name, next) in [("first", "second"), ("second", "read")] {
            facts.classes[0].methods.push(MethodTypeFact {
                name: name.into(),
                declaring_class: class.clone(),
                binding: MethodBinding::Instance,
                signature: CallableSignature {
                    parameters: Vec::new(),
                    return_type: StaticType::Unknown,
                    return_annotation_origin: AnnotationOrigin::Inferred,
                    uncertainty: BTreeSet::from([UncertaintyReason::Unknown]),
                },
                declared_final: false,
                override_policy: OverridePolicy::CompatibleSignatureRequired,
                implementation: Some(SourceIdentity {
                    module: facts.module.clone(),
                    lexical_qualname: format!("Box.{name}"),
                    source_range: SourceRange::new(
                        source.find(&format!("def {name}")).unwrap() as u32,
                        source.find(&format!("    def {next}")).unwrap() as u32,
                    ),
                    definition_kind: DefinitionKind::Function,
                }),
                generated: None,
                uncertainty: BTreeSet::from([UncertaintyReason::OpenWorld]),
            });
        }
        for method in &facts.classes[0].methods {
            let source = method.implementation.as_ref().unwrap();
            let function = module
                .callable_defs
                .iter_mut()
                .find(|function| function.names.qualname == source.lexical_qualname)
                .unwrap();
            function.scope.source_origin = Some(CallableSourceOrigin {
                definition: source.clone(),
                role: CallableSourceRole::SourceFunction,
            });
        }
        (facts, module, index)
    }

    #[test]
    fn strict_source_body_targets_require_source_identity_and_matching_native_abi() {
        let (facts, mut module, index) = method_fixture();
        let sites = method_sites(&facts, &module.callable_defs[index]);
        let targets = source_body_targets(&facts, &module.callable_defs);
        assert_eq!(targets.methods.len(), 2);
        assign_method_requests(
            &mut module.callable_defs[index],
            &module.module_constants,
            &sites,
        );
        let planner = SourceCallPlanner::new(
            &module.callable_defs[index],
            None,
            Arc::new(targets.clone()),
        )
        .unwrap();
        apply_source_call_plans(&mut module.callable_defs[index], planner);
        assert_eq!(
            checked_fixed_body_site_count(&module.callable_defs[index]),
            2
        );
        let target = targets.methods.values().next().unwrap();
        let target_index = module
            .callable_defs
            .iter()
            .position(|function| function.function_id == target.function_id)
            .unwrap();
        let original = module.callable_defs[target_index].clone();
        for variant in 0..4 {
            let function = &mut module.callable_defs[target_index];
            match variant {
                0 => {
                    function
                        .scope
                        .source_origin
                        .as_mut()
                        .unwrap()
                        .definition
                        .source_range
                        .end -= 1
                }
                1 => {
                    function.scope.source_origin.as_mut().unwrap().role =
                        CallableSourceRole::TypeParameterScope
                }
                2 => function.params.params[1].kind = ParamKind::KwOnly,
                _ => function.params.params[1].kind = ParamKind::VarArg,
            }
            assert_eq!(
                source_body_targets(&facts, &module.callable_defs)
                    .methods
                    .len(),
                1
            );
            module.callable_defs[target_index] = original.clone();
        }
        module.callable_defs.push(original);
        assert_eq!(
            source_body_targets(&facts, &module.callable_defs)
                .methods
                .len(),
            1,
            "ambiguous duplicate source identities never select a native target"
        );
    }

    #[test]
    fn sealed_method_requests_keep_family_and_field_layouts_separate() {
        let (mut facts, module, index) = method_fixture();
        let function = &module.callable_defs[index];
        let sites = method_sites(&facts, function);
        assert_eq!(
            sites
                .iter()
                .map(|site| (site.name.as_str(), site.capability_slot))
                .collect::<Vec<_>>(),
            [("second", 0), ("first", 1)]
        );
        assert!(field_sites(&facts, function).is_empty());
        facts.attribute_sites.reverse();
        assert_eq!(method_sites(&facts, function), sites);
        // Relational Self/unknown receiver predictions may propose a guard,
        // but the runtime must still supply the actual construction's family.
        for site in &mut facts.attribute_sites {
            site.receiver_type = StaticType::Any;
        }
        assert_eq!(method_sites(&facts, function), sites);
        facts.classes[0].methods[0].binding = MethodBinding::Static;
        assert_eq!(method_sites(&facts, function).len(), 1);
        facts.classes[0].methods[0].binding = MethodBinding::Instance;
        let mut field = fixture().0.classes[0].instance_fields[0].clone();
        field.declaring_class = facts.classes[0].methods[0].declaring_class.clone();
        field.field_kind = FieldKind::CallableInstanceField;
        facts.classes[0].instance_fields.push(field);
        assert_eq!(
            method_sites(&facts, function)
                .iter()
                .map(|site| site.name.as_str())
                .collect::<Vec<_>>(),
            ["second"]
        );
    }

    #[test]
    fn foreign_member_requests_stay_optional_and_do_not_mix_storage_and_method_slots() {
        let foreign = ModuleTypeFacts::new(
            "foreign_boxes",
            b"class Box: pass\n",
            SourceDialect::SoacStrict,
            ResolvedStrictPolicy {
                checked_attr: true,
                ..Default::default()
            },
        )
        .unwrap();
        let (mut facts, mut module, index) = method_fixture();
        let signature = facts.classes[0].methods[0].signature.clone();
        facts.classes.clear();
        for site in &mut facts.attribute_sites {
            let class = site.declaring_class.as_mut().unwrap();
            class.definition.module = foreign.module.clone();
            class.source_digest = foreign.source_digest;
            site.receiver_type = StaticType::NominalClass(class.clone());
            site.value_type = Some(StaticType::Callable(Box::new(signature.clone())));
        }
        let function = &mut module.callable_defs[index];
        let methods = method_sites(&facts, function);
        assert_eq!(methods.len(), 2);
        assert!(field_sites(&facts, function).is_empty());
        assign_method_requests(function, &module.module_constants, &methods);
        validate_method_requests_against_sites(function, &module.module_constants, &methods)
            .unwrap();
        assert_eq!(sealed_method_site_count(function), 2);
        for site in &mut facts.attribute_sites {
            site.value_type = Some(StaticType::Unknown);
        }
        assert!(method_sites(&facts, function).is_empty());
        assert_eq!(field_sites(&facts, function).len(), 2);
        // Neither request constructs a capability, promotes a result type, or
        // treats the unknown dependency as an exact class representation.
        for (_, _, result) in getters(function) {
            assert!(result.is_none_or(|facts| facts == ValueFacts::unknown_pyobj()));
        }
    }

    #[test]
    fn source_method_calls_preserve_bound_arity_without_promoting_expressions() {
        let (facts, mut module, index) = method_fixture();
        let sites = method_sites(&facts, &module.callable_defs[index]);
        assign_method_requests(
            &mut module.callable_defs[index],
            &module.module_constants,
            &sites,
        );
        let function = &mut module.callable_defs[index];
        let planner = SourceCallPlanner::new(function, None, Arc::default()).unwrap();
        apply_source_call_plans(function, planner);
        struct Plans(Vec<TypedSourceCallPlan>);
        impl Visit<InstrTyped> for Plans {
            fn visit_instr(&mut self, expression: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expression
                    && let Some(plan) = &call.extra.source_call
                {
                    self.0.push(plan.as_ref().clone());
                }
                expression.visit_children(self);
            }
        }
        let mut selected = Plans(Vec::new());
        selected.visit_fn(function);
        assert_eq!(
            selected
                .0
                .iter()
                .map(|plan| plan.argument_count)
                .collect::<Vec<_>>(),
            [2, 2]
        );
        assert!(
            validate_source_call_requests(function, None, Arc::default()).is_err(),
            "logical projection cannot mint an authenticated call capability"
        );
        soac_opt::passes::linearize_typed_function_expressions(function).unwrap();
        let mut linearized = Plans(Vec::new());
        linearized.visit_fn(function);
        assert_eq!(
            linearized.0, selected.0,
            "atomic call regions retain source identity and ordinary binding arity"
        );
    }

    #[test]
    fn sealed_method_requests_do_not_cross_an_inline_function_environment() {
        use soac_core::block_py::HasSemanticInstrId;
        use soac_ir_typed::{
            TypedDirectCallArgPlan, TypedDirectCallArgSource, TypedDirectFunctionCallGuard,
        };
        let (facts, mut module, index) = method_fixture();
        let sites = method_sites(&facts, &module.callable_defs[index]);
        assign_method_requests(
            &mut module.callable_defs[index],
            &module.module_constants,
            &sites,
        );
        let planner =
            SourceCallPlanner::new(&module.callable_defs[index], None, Arc::default()).unwrap();
        apply_source_call_plans(&mut module.callable_defs[index], planner);
        let target = module.callable_defs[index].function_id;
        let caller_source = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(fn, owner, value, argument):\n    result = fn(owner, value, argument)\n    return result\n",
        ).unwrap().blockpy_module;
        let mut caller_module = soac_ir_typed::lower_blockpy_module_to_typed(caller_source);
        let caller = caller_module
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "caller")
            .unwrap();
        // This inspection fixture uses no caller module constants. Give its
        // explicit call body a distinct function coordinate in the callee's
        // module, as the normal same-module inliner expects.
        caller.function_id = soac_core::block_py::RuntimeFunctionId::new(
            target.runtime_module_id(),
            soac_core::block_py::LocalFunctionId::new(999),
        );
        let argument_plan = TypedDirectCallArgPlan {
            sources: (0..3).map(TypedDirectCallArgSource::Provided).collect(),
        };
        struct Select<'a> {
            target: soac_core::block_py::RuntimeFunctionId,
            arguments: &'a TypedDirectCallArgPlan,
            call: Option<soac_core::block_py::InstrId>,
        }
        impl VisitMut<InstrTyped> for Select<'_> {
            fn visit_instr_mut(&mut self, instr: &mut InstrTyped) {
                if let InstrTyped::CallTyped(call) = instr {
                    self.call = call.try_semantic_instr_id();
                    call.access = TypedCallAccessPlan::GuardedCallable {
                        function_guards: vec![TypedDirectFunctionCallGuard {
                            function_id: self.target,
                            arg_plan: self.arguments.clone(),
                        }],
                    };
                }
                instr.visit_children_mut(self);
            }
        }
        let mut selected = Select {
            target,
            arguments: &argument_plan,
            call: None,
        };
        selected.visit_fn_mut(caller);
        let call = selected.call.unwrap();
        soac_opt::passes::lower_typed_function_call_access_plan_instrs(caller);
        let stats = soac_opt::passes::inline_typed_function_direct_call_stores(
            caller,
            &module,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::from([(call, vec![(target, argument_plan)])]),
        );
        assert_eq!(
            stats.rewritten_stores, 1,
            "the helper remains inline eligible"
        );
        assert_eq!(sealed_method_site_count(caller), 0);
        assert_eq!(sealed_method_site_count(&module.callable_defs[index]), 2);
        struct GenericMethods(usize);
        impl Visit<InstrTyped> for GenericMethods {
            fn visit_instr(&mut self, instr: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = instr
                    && matches!(call.func.as_ref(), InstrTyped::GetAttrTyped(_))
                {
                    assert!(matches!(call.access, TypedCallAccessPlan::Generic));
                    assert!(
                        call.extra.source_call.is_none(),
                        "inlining cannot borrow the old activation's source call plans"
                    );
                    self.0 += 1;
                }
                instr.visit_children(self);
            }
        }
        let mut methods = GenericMethods(0);
        methods.visit_fn(caller);
        assert_eq!(
            methods.0, 2,
            "copied calls retain normal lookup and argument evaluation"
        );
    }

    #[test]
    fn sealed_method_region_preserves_lookup_before_argument_callbacks() {
        let (facts, mut module, index) = method_fixture();
        let sites = method_sites(&facts, &module.callable_defs[index]);
        assign_method_requests(
            &mut module.callable_defs[index],
            &module.module_constants,
            &sites,
        );
        assert_eq!(sealed_method_site_count(&module.callable_defs[index]), 2);
        soac_opt::passes::linearize_typed_function_expressions(&mut module.callable_defs[index])
            .unwrap();
        assert_eq!(sealed_method_site_count(&module.callable_defs[index]), 2);
        validate_method_requests_against_sites(
            &module.callable_defs[index],
            &module.module_constants,
            &sites,
        )
        .unwrap();
        struct ArgumentOrder(bool);
        impl Visit<InstrTyped> for ArgumentOrder {
            fn visit_instr(&mut self, expression: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expression
                    && let TypedCallAccessPlan::GuardedSealedMethod(plan) = &call.access
                    && plan.name == "second"
                {
                    self.0 = matches!(call.func.as_ref(), InstrTyped::GetAttrTyped(_))
                        && matches!(
                            &call.args[0],
                            CallArgPositional::Positional(InstrTyped::CallTyped(_))
                        );
                }
                expression.visit_children(self);
            }
        }
        let mut order = ArgumentOrder(false);
        order.visit_fn(&module.callable_defs[index]);
        assert!(
            order.0,
            "the callback remains after lookup inside its planned region"
        );
        assert!(
            validate_typed_capability_requests(
                &module.callable_defs[index],
                None,
                &module.module_constants,
                &module.callable_defs,
            )
            .is_err()
        );
        for variant in 0..4 {
            let mut forged = sites.clone();
            match variant {
                0 => forged[0].name = "other".into(),
                1 => forged[0].site.expression_range.end -= 1,
                2 => forged[0].receiver_class.definition.lexical_qualname = "Unrelated".into(),
                _ => forged[0].capability_slot += 1,
            }
            assert!(
                validate_method_requests_against_sites(
                    &module.callable_defs[index],
                    &module.module_constants,
                    &forged
                )
                .is_err()
            );
        }
    }

    fn getters(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
    ) -> Vec<(InstrId, TypedAttrAccessPlan, Option<ValueFacts>)> {
        #[derive(Default)]
        struct Collector(Vec<(InstrId, TypedAttrAccessPlan, Option<ValueFacts>)>);
        impl Visit<InstrTyped> for Collector {
            fn visit_instr(&mut self, expression: &InstrTyped) {
                if let InstrTyped::GetAttrTyped(getter) = expression {
                    self.0.push((
                        getter.semantic_instr_id(),
                        getter.access.clone(),
                        getter.extra.result_facts(),
                    ));
                }
                expression.visit_children(self);
            }
        }
        let mut collector = Collector::default();
        collector.visit_fn(function);
        collector.0
    }

    #[test]
    fn sealed_field_requests_are_source_scoped_deterministic_optional_slots() {
        let (mut facts, module, index) = fixture();
        let function = &module.callable_defs[index];
        let plans = field_sites(&facts, function);
        assert_eq!(
            plans
                .iter()
                .map(|plan| (plan.name.as_str(), plan.capability_slot))
                .collect::<Vec<_>>(),
            vec![("second", 0), ("first", 1)]
        );
        facts.attribute_sites.reverse();
        assert_eq!(field_sites(&facts, function), plans);
        let original = facts.attribute_sites[0].clone();
        for access in [AttributeAccess::Write, AttributeAccess::Delete] {
            facts.attribute_sites[0].access = access;
            assert_eq!(field_sites(&facts, function).len(), 1);
        }
        facts.attribute_sites[0] = original.clone();
        facts.attribute_sites[0]
            .uncertainty
            .insert(UncertaintyReason::IgnoredDiagnostic);
        assert_eq!(field_sites(&facts, function).len(), 2);
        facts.attribute_sites[0] = original.clone();
        facts.attribute_sites[0].receiver_type = StaticType::Any;
        assert_eq!(field_sites(&facts, function).len(), 2);
        facts.attribute_sites[0].declaring_class = None;
        assert_eq!(field_sites(&facts, function).len(), 1);
        facts.attribute_sites[0] = original;
        facts.attribute_sites[0]
            .identity
            .enclosing_function
            .lexical_qualname = "other".into();
        assert_eq!(field_sites(&facts, function).len(), 1);
    }

    #[test]
    fn sealed_field_requests_do_not_promote_helpers_suspended_or_unowned_functions() {
        let (facts, mut module, index) = fixture();
        let function = &mut module.callable_defs[index];
        for role in [
            CallableSourceRole::ModuleBody,
            CallableSourceRole::ClassNamespace,
            CallableSourceRole::ClassConstruction,
            CallableSourceRole::AnnotationProvider,
        ] {
            function.scope.source_origin.as_mut().unwrap().role = role;
            assert!(field_sites(&facts, function).is_empty());
        }
        function.scope.source_origin.as_mut().unwrap().role = CallableSourceRole::SourceFunction;
        for kind in [
            FunctionKind::Generator,
            FunctionKind::Coroutine,
            FunctionKind::AsyncGenerator,
        ] {
            function.kind = kind;
            assert!(field_sites(&facts, function).is_empty());
        }
        function.kind = FunctionKind::Function;
        function.scope.source_origin = None;
        assert!(field_sites(&facts, function).is_empty());
    }

    #[test]
    fn sealed_field_request_validation_uses_actual_name_range_class_and_slot() {
        let (facts, mut module, index) = fixture();
        let function = &mut module.callable_defs[index];
        let sites = field_sites(&facts, function);
        let original_ids = getters(function)
            .into_iter()
            .map(|getter| getter.0)
            .collect::<Vec<_>>();
        assign_field_requests(function, &module.module_constants, &sites);
        assert_eq!(sealed_field_site_count(function), 2);
        validate_field_requests_against_sites(function, &module.module_constants, &sites).unwrap();
        assert_eq!(
            getters(function)
                .into_iter()
                .map(|getter| getter.0)
                .collect::<Vec<_>>(),
            original_ids
        );
        assert!(
            validate_typed_capability_requests(function, None, &module.module_constants, &[])
                .is_err(),
            "a syntactically valid request is not executable authority"
        );
        for variant in 0..4 {
            let mut changed_sites = sites.clone();
            let changed = &mut changed_sites[0];
            match variant {
                0 => changed.name = "different".into(),
                1 => changed.site.expression_range.end -= 1,
                2 => changed.receiver_class.definition.lexical_qualname = "Other".into(),
                _ => changed.capability_slot += 1,
            }
            assert!(
                validate_field_requests_against_sites(
                    function,
                    &module.module_constants,
                    &changed_sites
                )
                .is_err()
            );
        }
    }

    #[test]
    fn sealed_field_request_sync_removes_stale_result_fact_without_new_value_proofs() {
        let (facts, mut module, index) = fixture();
        let function_id = module.callable_defs[index].function_id;
        let sites = field_sites(&facts, &module.callable_defs[index]);
        let mut value_facts = FactStore::default();
        let stronger = ValueFacts::PyObj(PyObjFacts::exact_type(PyExactType::Int));
        for (id, _, _) in getters(&module.callable_defs[index]) {
            value_facts.insert_expr_fact(InstrKey::new(function_id, id), stronger);
        }
        assign_field_requests(
            &mut module.callable_defs[index],
            &module.module_constants,
            &sites,
        );
        soac_opt::passes::sync_typed_module_value_facts(&module, &mut value_facts);
        soac_opt::passes::annotate_typed_function_value_facts(
            &mut module.callable_defs[index],
            &value_facts,
        );
        assert!(
            getters(&module.callable_defs[index])
                .iter()
                .all(|(_, access, fact)| {
                    matches!(access, TypedAttrAccessPlan::GuardedSealedField(_))
                        && *fact == Some(ValueFacts::unknown_pyobj())
                })
        );
        // If stronger facts have already reached the IR, do not replace an
        // access after transformations may have relied on those facts.
        let (_, mut stronger_module, stronger_index) = fixture();
        let stronger_function_id = stronger_module.callable_defs[stronger_index].function_id;
        for (id, _, _) in getters(&stronger_module.callable_defs[stronger_index]) {
            value_facts.insert_expr_fact(InstrKey::new(stronger_function_id, id), stronger);
        }
        soac_opt::passes::annotate_typed_function_value_facts(
            &mut stronger_module.callable_defs[stronger_index],
            &value_facts,
        );
        assign_field_requests(
            &mut stronger_module.callable_defs[stronger_index],
            &stronger_module.module_constants,
            &sites,
        );
        assert_eq!(
            sealed_field_site_count(&stronger_module.callable_defs[stronger_index]),
            0
        );
    }
}
