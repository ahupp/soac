use std::collections::{BTreeMap, BTreeSet};

use crate::artifact::{canonicalize_dependencies, validate_dependencies};
use crate::*;

pub(crate) fn canonicalize_module(facts: &mut ModuleTypeFacts) -> Result<(), ContractError> {
    facts.language_policy.class_overrides.sort();
    canonicalize_dependencies(&mut facts.consumed_dependencies);
    facts
        .global_bindings
        .sort_by(|left, right| left.name.cmp(&right.name));
    for binding in &mut facts.global_bindings {
        binding.value_type = binding.value_type.normalized()?;
    }
    facts
        .classes
        .sort_by(|left, right| left.identity.cmp(&right.identity));
    for class in &mut facts.classes {
        // Field, base, MRO, decorator, and parameter ordering is semantic.
        // Only unordered catalogs and union members are sorted here.
        for field in &mut class.instance_fields {
            field.value_type = field.value_type.normalized()?;
            field.default.normalize_at_depth(0)?;
            normalize_descriptor(&mut field.descriptor)?;
        }
        class
            .methods
            .sort_by(|left, right| left.name.cmp(&right.name));
        for method in &mut class.methods {
            method.signature.normalize_at_depth(0)?;
        }
        class
            .class_members
            .sort_by(|left, right| left.name.cmp(&right.name));
        for member in &mut class.class_members {
            member.value_type = member.value_type.normalized()?;
            normalize_descriptor(&mut member.descriptor)?;
        }
    }
    facts
        .functions
        .sort_by(|left, right| left.identity.cmp(&right.identity));
    for function in &mut facts.functions {
        function.signature.normalize_at_depth(0)?;
    }
    facts.nominal_bindings.sort();
    facts
        .attribute_sites
        .sort_by(|left, right| left.identity.cmp(&right.identity));
    for site in &mut facts.attribute_sites {
        site.receiver_type = site.receiver_type.normalized()?;
        site.value_type = site
            .value_type
            .as_ref()
            .map(StaticType::normalized)
            .transpose()?;
    }
    facts
        .call_sites
        .sort_by(|left, right| left.identity.cmp(&right.identity));
    for site in &mut facts.call_sites {
        if let Some(receiver) = &mut site.receiver {
            receiver.value_type = receiver.value_type.normalized()?;
        }
        site.candidate_targets.sort();
        site.candidate_targets.dedup();
        site.signature.normalize_at_depth(0)?;
        site.result_type = site.result_type.normalized()?;
    }
    for diagnostic in &mut facts.diagnostics {
        diagnostic.related_definitions.sort();
        diagnostic.related_definitions.dedup();
    }
    facts.diagnostics.sort();
    crate::diagnostics::demote_suppressed_regions(facts);
    Ok(())
}

fn normalize_descriptor(descriptor: &mut DescriptorFact) -> Result<(), ContractError> {
    if let Some(value_type) = &mut descriptor.descriptor_type {
        **value_type = value_type.normalized()?;
    }
    Ok(())
}

/// Validate a proposal's schema, source identities, semantic categories, and
/// references. If source bytes are supplied, also verify both source hashes
/// and UTF-8 byte boundaries. This function does not authenticate unsigned
/// input or establish any runtime enforcement capability.
pub fn validate_module_facts(
    facts: &ModuleTypeFacts,
    source: Option<&[u8]>,
) -> Result<(), ContractError> {
    if facts.schema_version != ARTIFACT_SCHEMA_VERSION {
        return Err(ContractError::VersionMismatch {
            kind: "module shard schema",
            expected: ARTIFACT_SCHEMA_VERSION,
            found: facts.schema_version,
        });
    }
    validate_module_name(&facts.module.module_name)?;
    validate_dependencies(&facts.consumed_dependencies, &facts.module.module_name)?;
    if (facts.source_dialect == SourceDialect::SoacStrict) != facts.language_policy.is_selected() {
        return Err(ContractError::InvalidPolicy(
            "source admission must match the resolved comment rules".into(),
        ));
    }
    let source = source
        .map(|source| {
            if usize::try_from(facts.source_size).ok() != Some(source.len())
                || Fingerprint::digest(source) != facts.source_digest
                || legacy_source_hash(source) != facts.module.source_hash
            {
                return Err(ContractError::SourceMismatch(
                    facts.module.module_name.clone(),
                ));
            }
            std::str::from_utf8(source).map_err(|_| {
                ContractError::InvalidSourceIdentity("SOAC source must be valid UTF-8".into())
            })
        })
        .transpose()?;

    let mut definitions = BTreeSet::from([facts.module_body_identity()]);
    let mut classes = BTreeMap::new();
    let mut functions = BTreeSet::new();
    for class in &facts.classes {
        if classes.insert(class.identity.clone(), class).is_some() {
            return structure("duplicate class source identity");
        }
        definitions.insert(class.identity.clone());
    }
    let mut class_rules = BTreeSet::new();
    for rule in &facts.language_policy.class_overrides {
        if !class_rules.insert(rule.class_range)
            || !facts
                .classes
                .iter()
                .any(|class| class.identity.source_range == rule.class_range)
        {
            return Err(ContractError::InvalidPolicy(
                "class rules must uniquely identify an actual source class".into(),
            ));
        }
    }
    for function in &facts.functions {
        if !functions.insert(function.identity.clone()) {
            return structure("duplicate function source identity");
        }
        definitions.insert(function.identity.clone());
    }
    // These records introduce lexical non-callable definitions. Function and
    // class references must instead resolve to their full semantic catalogs.
    for identity in facts
        .global_bindings
        .iter()
        .filter_map(|binding| binding.definition.as_ref())
        .chain(facts.classes.iter().flat_map(|class| {
            class
                .class_members
                .iter()
                .filter_map(|member| member.definition.as_ref())
        }))
        .chain(facts.classes.iter().flat_map(|class| {
            class
                .instance_fields
                .iter()
                .filter_map(|field| field.annotation_definition.as_ref())
        }))
        .chain(
            facts
                .nominal_bindings
                .iter()
                .map(|binding| &binding.binding),
        )
    {
        if identity.module == facts.module
            && !matches!(
                identity.definition_kind,
                DefinitionKind::Class | DefinitionKind::Function | DefinitionKind::Lambda
            )
        {
            definitions.insert(identity.clone());
        }
    }
    let context = Context {
        facts,
        source,
        definitions,
        classes,
        functions,
        dependencies: facts
            .consumed_dependencies
            .iter()
            .map(|dependency| (dependency.module.module_name.as_str(), dependency))
            .collect(),
    };

    let mut global_names = BTreeSet::new();
    for binding in &facts.global_bindings {
        validate_name(&binding.name)?;
        if !global_names.insert(&binding.name) {
            return structure("duplicate global binding");
        }
        if let Some(identity) = &binding.definition {
            context.reference(identity)?;
        }
        context.static_type(&binding.value_type, 0)?;
        if !facts.language_policy.strict_assign && binding.mutability != GlobalMutability::Unknown {
            return structure("strict global restrictions require strict_assign");
        }
    }
    for class in &facts.classes {
        context.class(class)?;
    }
    context.inheritance_graph()?;
    for function in &facts.functions {
        context.local_definition(&function.identity)?;
        if !matches!(
            function.identity.definition_kind,
            DefinitionKind::Function | DefinitionKind::Lambda
        ) {
            return structure("function facts must identify a source function or lambda");
        }
        context.signature(&function.signature, 0)?;
        for decorator in &function.decorators {
            context.decorator(decorator)?;
        }
    }
    let mut nominal_leaves = BTreeSet::new();
    for binding in &facts.nominal_bindings {
        if !nominal_leaves.insert((&binding.owner, binding.expression_range)) {
            return structure("duplicate nominal annotation leaf");
        }
        context.nominal_binding(binding)?;
    }
    let mut attributes = BTreeSet::new();
    for site in &facts.attribute_sites {
        if !attributes.insert(&site.identity) {
            return structure("duplicate attribute site identity");
        }
        let identity = &site.identity;
        context.site(
            &identity.module,
            identity.source_digest,
            &identity.enclosing_function,
            identity.expression_range,
        )?;
        validate_name(&site.name)?;
        context.static_type(&site.receiver_type, 0)?;
        if let Some(value_type) = &site.value_type {
            context.static_type(value_type, 0)?;
        }
        if let Some(class) = &site.declaring_class {
            context.class_reference(class)?;
        }
    }
    let mut calls = BTreeSet::new();
    for site in &facts.call_sites {
        if !calls.insert(&site.identity) {
            return structure("duplicate call site identity");
        }
        let identity = &site.identity;
        context.site(
            &identity.module,
            identity.source_digest,
            &identity.enclosing_function,
            identity.expression_range,
        )?;
        if let Some(receiver) = &site.receiver {
            context.static_type(&receiver.value_type, 0)?;
        }
        if let Some(name) = &site.attribute_name {
            validate_name(name)?;
        }
        for target in &site.candidate_targets {
            context.call_target(target, site.binding)?;
        }
        if site.uncertainty == CallUncertainty::ExactStaticTarget
            && (site.candidate_targets.len() != 1
                || site.candidate_targets.first() == Some(&CallableTargetFact::Dynamic)
                || matches!(
                    site.binding,
                    CallBindingFact::Dynamic | CallBindingFact::Descriptor
                )
                || site.receiver.as_ref().is_some_and(|receiver| {
                    receiver.value_type.contains_uncertainty() || !receiver.uncertainty.is_empty()
                }))
        {
            return structure("an exact logical call target requires one non-dynamic target");
        }
        if matches!(
            site.binding,
            CallBindingFact::BoundInstanceMethod
                | CallBindingFact::BoundClassMethod
                | CallBindingFact::CallableInstanceField
        ) && (site.receiver.is_none() || site.attribute_name.is_none())
        {
            return structure("a bound attribute call requires a receiver and member name");
        }
        if site.uncertainty == CallUncertainty::CallableInstanceField
            && site.binding != CallBindingFact::CallableInstanceField
        {
            return structure("callable instance fields must not acquire method receiver binding");
        }
        context.signature(&site.signature, 0)?;
        context.static_type(&site.result_type, 0)?;
    }
    for diagnostic in &facts.diagnostics {
        context.local_range(diagnostic.source_range)?;
        match &diagnostic.scope {
            DiagnosticScope::Module => {}
            DiagnosticScope::Definition(identity) => {
                context.local_definition(identity)?;
                context.reference(identity)?;
                if !identity.source_range.contains(diagnostic.source_range) {
                    return Err(ContractError::InvalidSourceIdentity(
                        "diagnostic is outside its definition scope".into(),
                    ));
                }
            }
            DiagnosticScope::Site(range) => {
                context.local_range(*range)?;
                if !range.contains(diagnostic.source_range) {
                    return Err(ContractError::InvalidSourceIdentity(
                        "diagnostic is outside its site scope".into(),
                    ));
                }
            }
        }
        for definition in &diagnostic.related_definitions {
            context.reference(definition)?;
        }
        // A write can target a checked field declared in another module.
        // Its owner's policy, not the writer's module default, selects the
        // diagnostic. Referenced identities are validated above.
        if diagnostic.severity == DiagnosticSeverity::Error && !diagnostic.suppressed {
            return Err(ContractError::BlockingDiagnostic(format!(
                "{:?} at {}..{} (suppressed={})",
                diagnostic.code,
                diagnostic.source_range.start,
                diagnostic.source_range.end,
                diagnostic.suppressed,
            )));
        }
    }
    if facts
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.suppressed)
    {
        let mut dynamic = facts.clone();
        crate::diagnostics::demote_suppressed_regions(&mut dynamic);
        if dynamic != *facts {
            return Err(ContractError::BlockingDiagnostic(
                "suppressed regions retain precise class, value, or call proposals".into(),
            ));
        }
    }
    Ok(())
}

struct Context<'a> {
    facts: &'a ModuleTypeFacts,
    source: Option<&'a str>,
    definitions: BTreeSet<SourceIdentity>,
    classes: BTreeMap<SourceIdentity, &'a ClassTypeFact>,
    functions: BTreeSet<SourceIdentity>,
    dependencies: BTreeMap<&'a str, &'a DependencyFingerprint>,
}

impl Context<'_> {
    fn nominal_binding(&self, binding: &NominalBindingFact) -> Result<(), ContractError> {
        let (annotation_definition, value_type, origin) = match &binding.owner {
            NominalBindingOwner::Function {
                function,
                annotation,
            } => {
                self.function_reference(function)?;
                let fact = self
                    .facts
                    .functions
                    .iter()
                    .find(|fact| &fact.identity == function)
                    .ok_or_else(|| {
                        ContractError::InvalidStructure(
                            "nominal function owner is not local".into(),
                        )
                    })?;
                let (value_type, origin) = match annotation {
                    AnnotationTarget::Parameter { index } => {
                        let Some(parameter) = fact.signature.parameters.get(*index as usize) else {
                            return structure("nominal annotation parameter index is invalid");
                        };
                        (&parameter.value_type, parameter.annotation_origin)
                    }
                    AnnotationTarget::Return => (
                        &fact.signature.return_type,
                        fact.signature.return_annotation_origin,
                    ),
                };
                (function, value_type, origin)
            }
            NominalBindingOwner::Field { field } => {
                let fact = self.field_reference(field)?;
                (
                    &field.annotation_definition,
                    &fact.value_type,
                    fact.annotation_origin,
                )
            }
        };
        if annotation_definition.module != self.facts.module {
            return structure("nominal annotation belongs to another module");
        }
        self.local_range(binding.expression_range)?;
        if binding.expression_range.start == binding.expression_range.end
            || !annotation_definition
                .source_range
                .contains(binding.expression_range)
        {
            return structure("nominal annotation leaf is outside its source definition");
        }
        validate_name(&binding.name)?;
        if self.source.is_some_and(|source| {
            &source[binding.expression_range.start as usize..binding.expression_range.end as usize]
                != binding.name
        }) {
            return structure("nominal annotation name does not match its source bytes");
        }
        self.class_reference(&binding.class)?;
        self.reference(&binding.binding)?;
        self.reference(&binding.binding_scope)?;
        if binding.binding.module != self.facts.module
            || binding.binding_scope.module != self.facts.module
            || !matches!(
                binding.binding.definition_kind,
                DefinitionKind::Class | DefinitionKind::Assignment
            )
            || !matches!(
                binding.binding_scope.definition_kind,
                DefinitionKind::Module | DefinitionKind::Class | DefinitionKind::Function
            )
            || binding.binding_scope == binding.binding
            || !binding
                .binding_scope
                .source_range
                .contains(binding.binding.source_range)
            || !binding
                .binding_scope
                .source_range
                .contains(annotation_definition.source_range)
            || &binding.binding_scope == annotation_definition
        {
            return structure("nominal annotation has an invalid lexical binding scope");
        }
        // Exact lexical ownership prevents an outer class or function from
        // claiming a nested scope's alias merely because its range contains it.
        if self
            .facts
            .classes
            .iter()
            .map(|class| &class.identity)
            .chain(
                self.facts
                    .functions
                    .iter()
                    .map(|function| &function.identity),
            )
            .any(|scope| {
                scope != &binding.binding_scope
                    && scope != &binding.binding
                    && scope.source_range != binding.binding_scope.source_range
                    && binding
                        .binding_scope
                        .source_range
                        .contains(scope.source_range)
                    && scope.source_range.contains(binding.binding.source_range)
            })
        {
            return structure("nominal binding scope skips its actual lexical owner");
        }
        if binding.binding_scope.definition_kind == DefinitionKind::Module
            && !self.facts.global_bindings.iter().any(|global| {
                global.name == binding.name && global.definition.as_ref() == Some(&binding.binding)
            })
        {
            return structure("nominal global binding does not match the module catalog");
        }
        if matches!(&binding.owner, NominalBindingOwner::Field { .. })
            && binding.binding_scope.definition_kind == DefinitionKind::Class
            && self.facts.functions.iter().any(|function| {
                binding
                    .binding_scope
                    .source_range
                    .contains(function.identity.source_range)
                    && function
                        .identity
                        .source_range
                        .contains(annotation_definition.source_range)
            })
        {
            return structure(
                "a method-local field annotation cannot capture a class namespace alias",
            );
        }
        if origin != AnnotationOrigin::Explicit
            || !value_type.has_supported_value_shape()
            || !contains_nominal_class(value_type, &binding.class)
        {
            return structure(
                "nominal annotation leaf does not match an explicit supported contract",
            );
        }
        Ok(())
    }

    fn field_annotation_definition(&self, field: &FieldTypeFact) -> Result<(), ContractError> {
        let Some(definition) = &field.annotation_definition else {
            return Ok(());
        };
        self.reference(definition)?;
        if definition.definition_kind != DefinitionKind::Assignment
            || definition.module != field.declaring_class.definition.module
            || definition.source_range == field.declaring_class.definition.source_range
            || !field
                .declaring_class
                .definition
                .source_range
                .contains(definition.source_range)
        {
            return structure(
                "field annotation must identify an assignment in its declaring class",
            );
        }
        if definition.module == self.facts.module
            && self.facts.classes.iter().any(|class| {
                class.identity != field.declaring_class.definition
                    && field
                        .declaring_class
                        .definition
                        .source_range
                        .contains(class.identity.source_range)
                    && class
                        .identity
                        .source_range
                        .contains(definition.source_range)
            })
        {
            return structure("field annotation skips its actual declaring class");
        }
        Ok(())
    }

    fn field_reference(&self, reference: &FieldReference) -> Result<&FieldTypeFact, ContractError> {
        self.class_reference(&reference.declaring_class)?;
        self.reference(&reference.annotation_definition)?;
        validate_name(&reference.name)?;
        let Some(class) = self.classes.get(&reference.declaring_class.definition) else {
            return structure("nominal field owner is not a local declaring class");
        };
        let Some(field) = class.instance_fields.iter().find(|field| {
            field.name == reference.name
                && field.declaring_class == reference.declaring_class
                && field.annotation_definition.as_ref() == Some(&reference.annotation_definition)
        }) else {
            return structure("nominal field owner does not match its exact declaration catalog");
        };
        self.field_annotation_definition(field)?;
        Ok(field)
    }

    fn local_range(&self, range: SourceRange) -> Result<(), ContractError> {
        validate_range(range, self.facts.source_size)?;
        if let Some(source) = self.source {
            if !source.is_char_boundary(range.start as usize)
                || !source.is_char_boundary(range.end as usize)
            {
                return Err(ContractError::InvalidSourceIdentity(
                    "source range splits a UTF-8 code point".into(),
                ));
            }
        }
        Ok(())
    }

    fn local_definition(&self, identity: &SourceIdentity) -> Result<(), ContractError> {
        if identity.module != self.facts.module {
            return Err(ContractError::InvalidSourceIdentity(
                "source definition belongs to a different module".into(),
            ));
        }
        validate_qualname(&identity.lexical_qualname)?;
        self.local_range(identity.source_range)?;
        validate_definition_range(identity)?;
        Ok(())
    }

    fn reference(&self, identity: &SourceIdentity) -> Result<(), ContractError> {
        validate_qualname(&identity.lexical_qualname)?;
        validate_definition_range(identity)?;
        if identity.module == self.facts.module {
            self.local_range(identity.source_range)?;
            if !self.definitions.contains(identity) {
                return Err(ContractError::InvalidSourceIdentity(format!(
                    "unresolved local definition {}",
                    identity.lexical_qualname
                )));
            }
        } else {
            let dependency = self
                .dependencies
                .get(identity.module.module_name.as_str())
                .ok_or_else(|| {
                    ContractError::DependencyMismatch(identity.module.module_name.clone())
                })?;
            if identity.module != dependency.module {
                return Err(ContractError::DependencyMismatch(
                    identity.module.module_name.clone(),
                ));
            }
            validate_range(identity.source_range, dependency.source_size)?;
        }
        Ok(())
    }

    fn class_reference(&self, class: &ClassReference) -> Result<(), ContractError> {
        if class.definition.definition_kind != DefinitionKind::Class {
            return structure("class reference must identify a class definition");
        }
        self.reference(&class.definition)?;
        self.source_digest(&class.definition.module, class.source_digest)
    }

    fn source_digest(
        &self,
        module: &ModuleContentId,
        digest: Fingerprint,
    ) -> Result<(), ContractError> {
        let expected = if module == &self.facts.module {
            self.facts.source_digest
        } else {
            self.dependencies
                .get(module.module_name.as_str())
                .filter(|dependency| &dependency.module == module)
                .ok_or_else(|| ContractError::DependencyMismatch(module.module_name.clone()))?
                .source_digest
        };
        if expected != digest {
            return Err(ContractError::SourceMismatch(module.module_name.clone()));
        }
        Ok(())
    }

    fn function_reference(&self, identity: &SourceIdentity) -> Result<(), ContractError> {
        if !matches!(
            identity.definition_kind,
            DefinitionKind::Function | DefinitionKind::Lambda
        ) {
            return structure("callable implementation must identify a source function");
        }
        self.reference(identity)?;
        if identity.module == self.facts.module && !self.functions.contains(identity) {
            return structure("function reference has no function facts");
        }
        Ok(())
    }

    fn site(
        &self,
        module: &ModuleContentId,
        source_digest: Fingerprint,
        enclosing: &SourceIdentity,
        range: SourceRange,
    ) -> Result<(), ContractError> {
        if module != &self.facts.module || enclosing.module != self.facts.module {
            return structure("site and enclosing definition must belong to their shard");
        }
        self.source_digest(module, source_digest)?;
        self.reference(enclosing)?;
        if !matches!(
            enclosing.definition_kind,
            DefinitionKind::Module
                | DefinitionKind::Class
                | DefinitionKind::Function
                | DefinitionKind::Lambda
        ) {
            return structure("site must have a source execution owner");
        }
        self.local_range(range)?;
        if range.start == range.end || !enclosing.source_range.contains(range) {
            return structure("expression range is not inside its enclosing definition");
        }
        Ok(())
    }

    fn static_type(&self, value: &StaticType, depth: usize) -> Result<(), ContractError> {
        if depth > 64 {
            return Err(ContractError::InvalidType("type nesting exceeds 64".into()));
        }
        match value {
            StaticType::NominalClass(class) | StaticType::ExactClass(class) => {
                self.class_reference(class)?
            }
            StaticType::Union(elements) => {
                if elements.is_empty() {
                    return Err(ContractError::InvalidType("empty union".into()));
                }
                for element in elements {
                    self.static_type(element, depth + 1)?;
                }
            }
            StaticType::Optional(element) => self.static_type(element, depth + 1)?,
            StaticType::Callable(signature) => self.signature(signature, depth + 1)?,
            StaticType::Literal(literal) => validate_literal(literal)?,
            StaticType::TypeVariable(variable) => {
                // The binder itself is a source fact; unlike referenced
                // callable/class identities it need not have a runtime object.
                if variable.identity.module == self.facts.module {
                    self.local_definition(&variable.identity)?;
                } else {
                    self.reference(&variable.identity)?;
                }
                if !matches!(
                    variable.identity.definition_kind,
                    DefinitionKind::Parameter
                        | DefinitionKind::TypeAlias
                        | DefinitionKind::Assignment
                ) {
                    return structure("type variable requires a binder definition");
                }
                if let Some(bound) = &variable.upper_bound {
                    self.static_type(bound, depth + 1)?;
                }
                for constraint in &variable.constraints {
                    self.static_type(constraint, depth + 1)?;
                }
            }
            StaticType::StructuralProtocol(protocol) => {
                if let Some(class) = &protocol.definition {
                    self.class_reference(class)?;
                }
            }
            StaticType::NumericWidening { target, accepted } => {
                let expected = match target {
                    BuiltinType::Float => BTreeSet::from([BuiltinType::Int, BuiltinType::Float]),
                    BuiltinType::Complex => {
                        BTreeSet::from([BuiltinType::Int, BuiltinType::Float, BuiltinType::Complex])
                    }
                    _ => {
                        return Err(ContractError::InvalidType(
                            "unsupported numeric widening target".into(),
                        ));
                    }
                };
                if accepted != &expected {
                    return Err(ContractError::InvalidType(
                        "numeric widening must preserve typing's acceptance set".into(),
                    ));
                }
            }
            StaticType::None
            | StaticType::ExactBuiltin(_)
            | StaticType::NominalBuiltin { .. }
            | StaticType::Any
            | StaticType::Unknown
            | StaticType::Todo
            | StaticType::Divergent
            | StaticType::Unsupported { .. } => {}
        }
        Ok(())
    }

    fn signature(&self, signature: &CallableSignature, depth: usize) -> Result<(), ContractError> {
        if depth > 64 {
            return Err(ContractError::InvalidType(
                "signature nesting exceeds 64".into(),
            ));
        }
        let mut names = BTreeSet::new();
        let mut previous_kind = ParameterKind::PositionalOnly;
        let mut has_varargs = false;
        let mut has_varkw = false;
        let mut positional_default = false;
        for parameter in &signature.parameters {
            validate_name(&parameter.name)?;
            if !names.insert(&parameter.name) {
                return structure("duplicate signature parameter");
            }
            if parameter.kind < previous_kind {
                return structure("signature parameters are not in Python binding order");
            }
            previous_kind = parameter.kind;
            let has_default = parameter.default != DefaultFact::Missing;
            match parameter.kind {
                ParameterKind::VarArgs => {
                    if has_varargs || has_default {
                        return structure("invalid variadic positional parameter");
                    }
                    has_varargs = true;
                }
                ParameterKind::VarKeywords => {
                    if has_varkw || has_default {
                        return structure("invalid variadic keyword parameter");
                    }
                    has_varkw = true;
                }
                ParameterKind::PositionalOnly | ParameterKind::PositionalOrKeyword => {
                    if positional_default && !has_default {
                        return structure("required positional parameter follows a default");
                    }
                    positional_default |= has_default;
                }
                ParameterKind::KeywordOnly => {}
            }
            self.static_type(&parameter.value_type, depth + 1)?;
            self.default(&parameter.default, depth + 1)?;
        }
        self.static_type(&signature.return_type, depth + 1)
    }

    fn default(&self, default: &DefaultFact, depth: usize) -> Result<(), ContractError> {
        match default {
            DefaultFact::Value {
                value_type,
                literal,
            } => {
                self.static_type(value_type, depth + 1)?;
                if let Some(literal) = literal {
                    validate_literal(literal)?;
                }
            }
            DefaultFact::Factory {
                implementation,
                return_type,
            } => {
                if let Some(implementation) = implementation {
                    self.function_reference(implementation)?;
                }
                self.static_type(return_type, depth + 1)?;
            }
            DefaultFact::Missing | DefaultFact::Unknown => {}
        }
        Ok(())
    }

    fn descriptor(&self, descriptor: &DescriptorFact) -> Result<(), ContractError> {
        if let Some(value_type) = &descriptor.descriptor_type {
            self.static_type(value_type, 0)?;
        }
        for implementation in [&descriptor.getter, &descriptor.setter, &descriptor.deleter]
            .into_iter()
            .flatten()
        {
            self.function_reference(implementation)?;
        }
        if descriptor.kind == DescriptorKind::None
            && (descriptor.getter.is_some()
                || descriptor.setter.is_some()
                || descriptor.deleter.is_some())
        {
            return structure("non-descriptor cannot name descriptor accessors");
        }
        if matches!(
            descriptor.kind,
            DescriptorKind::NonData | DescriptorKind::StdlibCachedProperty
        ) && (descriptor.setter.is_some() || descriptor.deleter.is_some())
        {
            return structure("non-data descriptor cannot claim setter/deleter slots");
        }
        Ok(())
    }

    fn decorator(&self, decorator: &DecoratorFact) -> Result<(), ContractError> {
        self.local_range(decorator.expression_range)?;
        match (&decorator.definition, decorator.source_digest) {
            (Some(identity), Some(digest)) => {
                self.reference(identity)?;
                self.source_digest(&identity.module, digest)?;
            }
            (None, None) => {}
            _ => return structure("decorator definition and source digest must be paired"),
        }
        for (name, value) in &decorator.arguments {
            validate_name(name)?;
            validate_literal(value)?;
        }
        Ok(())
    }

    fn generated_function(&self, generated: &GeneratedFunctionFact) -> Result<(), ContractError> {
        self.class_reference(&generated.class)?;
        validate_name(&generated.name)?;
        if let Some(class) = self.classes.get(&generated.class.definition) {
            if class.transform.as_ref().is_none_or(|transform| {
                transform.kind != generated.transform
                    || !transform.generated_methods.contains(&generated.name)
            }) {
                return structure("generated function is absent from its class transform");
            }
        }
        Ok(())
    }

    fn class(&self, class: &ClassTypeFact) -> Result<(), ContractError> {
        self.local_definition(&class.identity)?;
        if class.identity.definition_kind != DefinitionKind::Class {
            return structure("class facts must identify a source class");
        }
        let mut direct_bases = BTreeSet::new();
        let mut mro_bases = BTreeSet::new();
        if class.bases.iter().any(|base| !direct_bases.insert(base))
            || class
                .inheritance
                .linearized_bases
                .iter()
                .any(|base| !mro_bases.insert(base))
        {
            return structure("duplicate direct base or MRO entry");
        }
        if class.inheritance.complete && !direct_bases.is_subset(&mro_bases) {
            return structure("complete logical MRO omits a direct base");
        }
        if class.inheritance.complete
            && class
                .inheritance
                .linearized_bases
                .iter()
                .position(|base| *base == BaseReference::Builtin(BuiltinType::Object))
                .is_some_and(|index| index + 1 != class.inheritance.linearized_bases.len())
        {
            return structure("builtin object must terminate a complete logical MRO");
        }
        for base in class
            .bases
            .iter()
            .chain(&class.inheritance.linearized_bases)
            .filter_map(BaseReference::as_class)
        {
            self.class_reference(base)?;
            if base.definition == class.identity {
                return structure("class must not appear in its own base list or MRO");
            }
        }
        if let MetaclassFact::Class(metaclass) = &class.metaclass {
            self.class_reference(metaclass)?;
        }
        for decorator in &class.decorators {
            self.decorator(decorator)?;
        }
        match &class.participation {
            ParticipationProposal::Candidate => {
                if !self
                    .facts
                    .language_policy
                    .checked_attributes(class.identity.source_range)
                {
                    return Err(ContractError::InvalidPolicy(
                        "a class opted out of checked_attr cannot propose participation".into(),
                    ));
                }
                if self.facts.source_dialect != SourceDialect::SoacStrict
                    || class.metaclass != MetaclassFact::BuiltinType
                    || !class.inheritance.complete
                {
                    return structure(
                        "candidate participation requires selected checked_attr, builtin type, and resolved inheritance",
                    );
                }
                if class.decorators.iter().any(|decorator| {
                    matches!(
                        decorator.kind,
                        DecoratorKind::Other
                            | DecoratorKind::Unknown
                            | DecoratorKind::DataclassTransform
                    ) || !decorator.uncertainty.is_empty()
                        || (decorator.kind == DecoratorKind::StdlibDataclass
                            && decorator.definition.is_none())
                }) {
                    return structure("unmodeled decorators must retain dynamic classification");
                }
                if class.uncertainty.iter().any(|reason| {
                    matches!(
                        reason,
                        UncertaintyReason::IgnoredDiagnostic
                            | UncertaintyReason::DynamicDecorator
                            | UncertaintyReason::DynamicMetaclass
                            | UncertaintyReason::UnsafeNarrowing
                    )
                }) {
                    return structure("uncertain class provenance cannot propose participation");
                }
                if class
                    .transform
                    .as_ref()
                    .is_some_and(|transform| transform.kind != TransformKind::StdlibDataclass)
                {
                    return structure("unadapted framework transforms must remain dynamic");
                }
            }
            ParticipationProposal::Dynamic(reasons) if reasons.is_empty() => {
                return structure("dynamic classification must retain its reason");
            }
            ParticipationProposal::Dynamic(_) => {}
        }
        let mut field_names = BTreeSet::new();
        for field in &class.instance_fields {
            validate_name(&field.name)?;
            if !field_names.insert(&field.name) {
                return structure("duplicate logical class field");
            }
            self.declaring_class(class, &field.declaring_class)?;
            self.field_annotation_definition(field)?;
            self.static_type(&field.value_type, 0)?;
            self.default(&field.default, 0)?;
            self.descriptor(&field.descriptor)?;
            if field.field_kind == FieldKind::InitOnly
                && field.write_policy != FieldWritePolicy::InitOnly
            {
                return structure("InitVar must retain its init-only field policy");
            }
        }
        let mut method_names = BTreeSet::new();
        for method in &class.methods {
            validate_name(&method.name)?;
            if !method_names.insert(&method.name) {
                return structure("duplicate logical method");
            }
            self.declaring_class(class, &method.declaring_class)?;
            self.signature(&method.signature, 0)?;
            if method.override_policy == OverridePolicy::DeclaredFinal && !method.declared_final {
                return structure("final override policy requires a declared-final method");
            }
            match (&method.implementation, &method.generated) {
                (Some(implementation), None) => self.function_reference(implementation)?,
                (None, Some(generated)) => {
                    self.generated_function(generated)?;
                    if generated.class != method.declaring_class || generated.name != method.name {
                        return structure(
                            "generated method does not match its declaring class and name",
                        );
                    }
                }
                (None, None) if !method.uncertainty.is_empty() => {}
                _ => {
                    return structure(
                        "method requires one implementation origin or explicit uncertainty",
                    );
                }
            }
        }
        let mut member_names = BTreeSet::new();
        for member in &class.class_members {
            validate_name(&member.name)?;
            if !member_names.insert(&member.name) {
                return structure("duplicate class member");
            }
            if let Some(identity) = &member.definition {
                self.reference(identity)?;
            }
            self.static_type(&member.value_type, 0)?;
            self.descriptor(&member.descriptor)?;
        }
        if let Some(transform) = &class.transform {
            if let Some(provenance) = &transform.provenance {
                self.reference(provenance)?;
            }
            if transform.kind == TransformKind::StdlibDataclass
                && transform.dataclass_options.is_none()
            {
                return structure("stdlib dataclass transform requires resolved options");
            }
            if let Some(options) = &transform.dataclass_options {
                if (options.weakref_slot && !options.slots) || (options.order && !options.eq) {
                    return structure("invalid stdlib dataclass options");
                }
            }
            for name in &transform.generated_methods {
                validate_name(name)?;
            }
        }
        Ok(())
    }

    fn declaring_class(
        &self,
        owner: &ClassTypeFact,
        declaring: &ClassReference,
    ) -> Result<(), ContractError> {
        self.class_reference(declaring)?;
        if declaring.definition != owner.identity
            && !owner
                .bases
                .iter()
                .chain(&owner.inheritance.linearized_bases)
                .any(|base| base.as_class() == Some(declaring))
        {
            return structure("member's declaring class is outside its logical inheritance");
        }
        Ok(())
    }

    fn inheritance_graph(&self) -> Result<(), ContractError> {
        let mut state = BTreeMap::new();
        for identity in self.classes.keys() {
            if state.get(identity) == Some(&2u8) {
                continue;
            }
            let mut pending = vec![(identity, false)];
            while let Some((identity, exiting)) = pending.pop() {
                if exiting {
                    state.insert(identity, 2u8);
                    continue;
                }
                match state.get(identity) {
                    Some(2) => continue,
                    Some(1) => return structure("cyclic logical class inheritance"),
                    _ => {}
                }
                state.insert(identity, 1u8);
                pending.push((identity, true));
                if let Some(class) = self.classes.get(identity) {
                    for base in class.bases.iter().rev().filter_map(BaseReference::as_class) {
                        if let Some((identity, _)) = self.classes.get_key_value(&base.definition) {
                            pending.push((identity, false));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn call_target(
        &self,
        target: &CallableTargetFact,
        binding: CallBindingFact,
    ) -> Result<(), ContractError> {
        match target {
            CallableTargetFact::SourceFunction(identity) => self.function_reference(identity)?,
            CallableTargetFact::Method {
                class,
                name,
                implementation,
            } => {
                self.class_reference(class)?;
                validate_name(name)?;
                if let Some(implementation) = implementation {
                    self.function_reference(implementation)?;
                }
                if let Some(class) = self.classes.get(&class.definition) {
                    let Some(method) = class.methods.iter().find(|method| &method.name == name)
                    else {
                        return structure("call target names an absent logical method");
                    };
                    if implementation.is_some() && implementation != &method.implementation {
                        return structure(
                            "call target implementation disagrees with its method facts",
                        );
                    }
                    let compatible = match binding {
                        CallBindingFact::UnboundFunction => {
                            matches!(
                                method.binding,
                                MethodBinding::Instance | MethodBinding::Static
                            )
                        }
                        CallBindingFact::BoundInstanceMethod => {
                            method.binding == MethodBinding::Instance
                        }
                        CallBindingFact::BoundClassMethod => method.binding == MethodBinding::Class,
                        CallBindingFact::StaticMethod => method.binding == MethodBinding::Static,
                        CallBindingFact::CallableInstanceField => false,
                        CallBindingFact::Descriptor => matches!(
                            method.binding,
                            MethodBinding::PropertyGetter | MethodBinding::Descriptor
                        ),
                        CallBindingFact::Dynamic => true,
                    };
                    if !compatible {
                        return structure("call binding disagrees with its logical method binding");
                    }
                }
            }
            CallableTargetFact::Generated(generated) => self.generated_function(generated)?,
            CallableTargetFact::Dynamic => {}
        }
        Ok(())
    }
}

fn contains_nominal_class(value_type: &StaticType, class: &ClassReference) -> bool {
    match value_type {
        StaticType::NominalClass(reference) | StaticType::ExactClass(reference) => {
            reference == class
        }
        StaticType::Union(elements) => elements
            .iter()
            .any(|element| contains_nominal_class(element, class)),
        StaticType::Optional(element) => contains_nominal_class(element, class),
        _ => false,
    }
}

fn structure<T>(message: &str) -> Result<T, ContractError> {
    Err(ContractError::InvalidStructure(message.into()))
}

pub(crate) fn validate_module_name(name: &str) -> Result<(), ContractError> {
    if name.len() > 4096
        || name
            .split('.')
            .any(|component| validate_name(component).is_err())
    {
        return Err(ContractError::InvalidSourceIdentity(
            "invalid canonical module name".into(),
        ));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), ContractError> {
    if name.is_empty()
        || name.len() > 4096
        || name
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | '.'))
    {
        return structure("invalid member or parameter name");
    }
    Ok(())
}

fn validate_qualname(name: &str) -> Result<(), ContractError> {
    if name.is_empty() || name.len() > 16384 || name.chars().any(char::is_control) {
        return Err(ContractError::InvalidSourceIdentity(
            "invalid lexical qualified name".into(),
        ));
    }
    Ok(())
}

fn validate_range(range: SourceRange, source_size: u32) -> Result<(), ContractError> {
    if range.start > range.end || range.end > source_size {
        return Err(ContractError::InvalidSourceIdentity(
            "byte range is outside its source".into(),
        ));
    }
    Ok(())
}

fn validate_definition_range(identity: &SourceIdentity) -> Result<(), ContractError> {
    if identity.definition_kind != DefinitionKind::Module
        && identity.source_range.start == identity.source_range.end
    {
        return Err(ContractError::InvalidSourceIdentity(
            "source definition has an empty range".into(),
        ));
    }
    Ok(())
}

fn validate_literal(literal: &LiteralValue) -> Result<(), ContractError> {
    if let LiteralValue::Int(value) = literal {
        let digits = value.strip_prefix('-').unwrap_or(value);
        if digits.is_empty()
            || !digits.bytes().all(|byte| byte.is_ascii_digit())
            || (digits.len() > 1 && digits.starts_with('0'))
            || value == "-0"
        {
            return Err(ContractError::InvalidType(
                "integer literal is not canonical decimal".into(),
            ));
        }
    }
    Ok(())
}
