//! Conservative loss of optional facts for checker-suppressed source regions.
//!
//! No suppression can confer authority. Keep lexical identities and the
//! diagnostic itself, but remove the affected class, value, and call proposals.
//! The encoder performs this lowering; the verifier requires it to have been
//! applied already so a signed producer cannot accidentally retain those facts.

use std::collections::BTreeSet;

use crate::*;

pub(crate) fn demote_suppressed_regions(facts: &mut ModuleTypeFacts) {
    if !facts
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.suppressed)
    {
        return;
    }
    let mut ignored = IgnoredDefinitions {
        module: facts.module.clone(),
        whole_module: false,
        definitions: BTreeSet::new(),
    };
    for diagnostic in facts
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.suppressed)
    {
        let range = match &diagnostic.scope {
            DiagnosticScope::Module => {
                ignored.whole_module = true;
                continue;
            }
            DiagnosticScope::Definition(identity) => identity.source_range,
            DiagnosticScope::Site(range) => *range,
        };
        let mut owner_found = false;
        for (identity, decorators) in facts
            .classes
            .iter()
            .map(|class| (&class.identity, &class.decorators))
            .chain(
                facts
                    .functions
                    .iter()
                    .map(|function| (&function.identity, &function.decorators)),
            )
        {
            if overlaps(identity.source_range, range)
                || decorators
                    .iter()
                    .any(|decorator| overlaps(decorator.expression_range, range))
            {
                ignored.definitions.insert(identity.clone());
                owner_found = true;
            }
        }
        // A top-level assignment/import can influence arbitrary later facts;
        // this schema has no variable-use dependency graph to prove otherwise.
        // Class/function regions, in contrast, have explicit lexical owners.
        ignored.whole_module |= !owner_found;
        ignored
            .definitions
            .extend(diagnostic.related_definitions.iter().cloned());
    }

    // A class owns its methods and nested definitions. Dependents of an
    // ignored base/decorator/implementation also cannot acquire its proposals.
    loop {
        let mut added = BTreeSet::new();
        for class in &facts.classes {
            let affected = ignored.identity(&class.identity)
                || class
                    .bases
                    .iter()
                    .chain(&class.inheritance.linearized_bases)
                    .filter_map(crate::BaseReference::as_class)
                    .any(|base| ignored.identity(&base.definition))
                || matches!(&class.metaclass, MetaclassFact::Class(class) if ignored.identity(&class.definition))
                || class.decorators.iter().any(|decorator| {
                    decorator
                        .definition
                        .as_ref()
                        .is_some_and(|identity| ignored.identity(identity))
                })
                || class.methods.iter().any(|method| {
                    method
                        .implementation
                        .as_ref()
                        .is_some_and(|identity| ignored.identity(identity))
                });
            if affected {
                added.insert(class.identity.clone());
            }
        }
        for function in &facts.functions {
            if ignored.identity(&function.identity)
                || function.decorators.iter().any(|decorator| {
                    decorator
                        .definition
                        .as_ref()
                        .is_some_and(|identity| ignored.identity(identity))
                })
            {
                added.insert(function.identity.clone());
            }
        }
        let before = ignored.definitions.len();
        ignored.definitions.extend(added);
        if before == ignored.definitions.len() {
            break;
        }
    }

    for binding in &mut facts.global_bindings {
        let full = ignored.whole_module
            || binding
                .definition
                .as_ref()
                .is_some_and(|identity| ignored.identity(identity));
        if ignored.value(&mut binding.value_type, full) {
            binding
                .uncertainty
                .insert(UncertaintyReason::IgnoredDiagnostic);
        }
    }
    for class in &mut facts.classes {
        let full = ignored.identity(&class.identity);
        if full {
            match &mut class.participation {
                ParticipationProposal::Candidate => {
                    class.participation = ParticipationProposal::Dynamic(BTreeSet::from([
                        DynamicClassReason::IgnoredDiagnostic,
                    ]));
                }
                ParticipationProposal::Dynamic(reasons) => {
                    reasons.insert(DynamicClassReason::IgnoredDiagnostic);
                }
            }
            class.dictionary = ClassDictionarySemantics::Unknown;
            class.inheritance.complete = false;
            class.openness = ClassOpenness::Unknown;
            class
                .uncertainty
                .insert(UncertaintyReason::IgnoredDiagnostic);
        }
        for field in &mut class.instance_fields {
            let field_full = full
                || ignored.identity(&field.declaring_class.definition)
                || field
                    .annotation_definition
                    .as_ref()
                    .is_some_and(|definition| ignored.identity(definition));
            let affected = ignored.value(&mut field.value_type, field_full)
                | ignored.default(&mut field.default, field_full)
                | ignored.descriptor(&mut field.descriptor, field_full);
            if field_full {
                field.field_kind = FieldKind::Dynamic;
                field.read_policy = FieldReadPolicy::PythonAttribute;
                field.write_policy = FieldWritePolicy::Dynamic;
                field.initialization = InitializationPolicy::Unknown;
            }
            if affected {
                field
                    .uncertainty
                    .insert(UncertaintyReason::IgnoredDiagnostic);
            }
        }
        for method in &mut class.methods {
            if ignored.signature(&mut method.signature, full) {
                method
                    .uncertainty
                    .insert(UncertaintyReason::IgnoredDiagnostic);
            }
            if full {
                method.override_policy = OverridePolicy::Dynamic;
            }
        }
        for member in &mut class.class_members {
            if ignored.value(&mut member.value_type, full)
                | ignored.descriptor(&mut member.descriptor, full)
            {
                member
                    .uncertainty
                    .insert(UncertaintyReason::IgnoredDiagnostic);
            }
            if full {
                member.kind = ClassMemberKind::Dynamic;
            }
        }
    }
    for function in &mut facts.functions {
        if ignored.signature(
            &mut function.signature,
            ignored.identity(&function.identity),
        ) {
            function
                .uncertainty
                .insert(UncertaintyReason::IgnoredDiagnostic);
        }
    }
    facts.nominal_bindings.retain(|binding| {
        let owner_ignored = match &binding.owner {
            NominalBindingOwner::Function { function, .. } => ignored.identity(function),
            NominalBindingOwner::Field { field } => {
                ignored.identity(&field.declaring_class.definition)
                    || ignored.identity(&field.annotation_definition)
            }
        };
        !owner_ignored
            && !ignored.identity(&binding.class.definition)
            && !ignored.identity(&binding.binding)
            && !ignored.identity(&binding.binding_scope)
    });
    for site in &mut facts.attribute_sites {
        if ignored.identity(&site.identity.enclosing_function)
            || ignored.value_references(&site.receiver_type)
            || site
                .value_type
                .as_ref()
                .is_some_and(|value| ignored.value_references(value))
            || site
                .declaring_class
                .as_ref()
                .is_some_and(|class| ignored.identity(&class.definition))
        {
            site.receiver_type = StaticType::Unknown;
            if site.value_type.is_some() {
                site.value_type = Some(StaticType::Unknown);
            }
            site.declaring_class = None;
            site.uncertainty
                .insert(UncertaintyReason::IgnoredDiagnostic);
        }
    }
    for site in &mut facts.call_sites {
        if ignored.identity(&site.identity.enclosing_function)
            || site
                .receiver
                .as_ref()
                .is_some_and(|receiver| ignored.value_references(&receiver.value_type))
            || site
                .candidate_targets
                .iter()
                .any(|target| ignored.target(target))
            || ignored.signature_references(&site.signature)
            || ignored.value_references(&site.result_type)
        {
            if let Some(receiver) = &mut site.receiver {
                receiver.value_type = StaticType::Unknown;
                receiver
                    .uncertainty
                    .insert(UncertaintyReason::IgnoredDiagnostic);
            }
            site.candidate_targets = vec![CallableTargetFact::Dynamic];
            site.binding = CallBindingFact::Dynamic;
            ignored.signature(&mut site.signature, true);
            site.result_type = StaticType::Unknown;
            site.uncertainty = CallUncertainty::Dynamic;
        }
    }
}

struct IgnoredDefinitions {
    module: ModuleContentId,
    whole_module: bool,
    definitions: BTreeSet<SourceIdentity>,
}

impl IgnoredDefinitions {
    fn identity(&self, identity: &SourceIdentity) -> bool {
        (self.whole_module && identity.module == self.module)
            || self.definitions.contains(identity)
            || self.definitions.iter().any(|ignored| {
                ignored.module == identity.module
                    && overlaps(ignored.source_range, identity.source_range)
                    && identity.definition_kind != DefinitionKind::Module
            })
    }

    fn value_references(&self, value: &StaticType) -> bool {
        match value {
            StaticType::NominalClass(class) | StaticType::ExactClass(class) => {
                self.identity(&class.definition)
            }
            StaticType::Union(elements) => {
                elements.iter().any(|value| self.value_references(value))
            }
            StaticType::Optional(value) => self.value_references(value),
            StaticType::Callable(signature) => self.signature_references(signature),
            StaticType::TypeVariable(variable) => {
                self.identity(&variable.identity)
                    || variable
                        .upper_bound
                        .as_ref()
                        .is_some_and(|value| self.value_references(value))
                    || variable
                        .constraints
                        .iter()
                        .any(|value| self.value_references(value))
            }
            StaticType::StructuralProtocol(protocol) => protocol
                .definition
                .as_ref()
                .is_some_and(|class| self.identity(&class.definition)),
            _ => false,
        }
    }

    fn signature_references(&self, signature: &CallableSignature) -> bool {
        self.value_references(&signature.return_type)
            || signature.parameters.iter().any(|parameter| {
                self.value_references(&parameter.value_type)
                    || self.default_references(&parameter.default)
            })
    }

    fn default_references(&self, default: &DefaultFact) -> bool {
        match default {
            DefaultFact::Value { value_type, .. } => self.value_references(value_type),
            DefaultFact::Factory {
                implementation,
                return_type,
            } => {
                implementation
                    .as_ref()
                    .is_some_and(|identity| self.identity(identity))
                    || self.value_references(return_type)
            }
            DefaultFact::Missing | DefaultFact::Unknown => false,
        }
    }

    fn target(&self, target: &CallableTargetFact) -> bool {
        match target {
            CallableTargetFact::SourceFunction(identity) => self.identity(identity),
            CallableTargetFact::Method {
                class,
                implementation,
                ..
            } => {
                self.identity(&class.definition)
                    || implementation
                        .as_ref()
                        .is_some_and(|identity| self.identity(identity))
            }
            CallableTargetFact::Generated(generated) => self.identity(&generated.class.definition),
            CallableTargetFact::Dynamic => false,
        }
    }

    fn value(&self, value: &mut StaticType, full: bool) -> bool {
        if full || self.value_references(value) {
            *value = StaticType::Unknown;
            true
        } else {
            false
        }
    }

    fn default(&self, default: &mut DefaultFact, full: bool) -> bool {
        if (full && *default != DefaultFact::Missing) || self.default_references(default) {
            *default = DefaultFact::Unknown;
            true
        } else {
            false
        }
    }

    fn signature(&self, signature: &mut CallableSignature, full: bool) -> bool {
        let mut affected = self.value(&mut signature.return_type, full);
        for parameter in &mut signature.parameters {
            affected |= self.value(&mut parameter.value_type, full);
            affected |= self.default(&mut parameter.default, full);
        }
        if affected {
            signature
                .uncertainty
                .insert(UncertaintyReason::IgnoredDiagnostic);
        }
        affected
    }

    fn descriptor(&self, descriptor: &mut DescriptorFact, full: bool) -> bool {
        if full
            || descriptor
                .descriptor_type
                .as_ref()
                .is_some_and(|value| self.value_references(value))
            || [&descriptor.getter, &descriptor.setter, &descriptor.deleter]
                .into_iter()
                .flatten()
                .any(|identity| self.identity(identity))
        {
            *descriptor = DescriptorFact {
                kind: DescriptorKind::Unknown,
                ..DescriptorFact::default()
            };
            true
        } else {
            false
        }
    }
}

fn overlaps(left: SourceRange, right: SourceRange) -> bool {
    if left.start == left.end {
        right.start <= left.start && left.start < right.end
    } else if right.start == right.end {
        left.start <= right.start && right.start < left.end
    } else {
        left.start < right.end && right.start < left.end
    }
}
