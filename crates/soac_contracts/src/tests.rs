use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::Signer;

use crate::artifact::canonical_bytes;
use crate::*;

const SOURCE: &[u8] = b"from __future__ import strict\n\nclass Independent:\n    pass\n\nclass Box:\n    value: int\n    def get(self) -> int:\n        return self.value\n\ndef read(box: Box) -> int:\n    return box.get()\n";
const SIGNING_SEED: [u8; 32] = [19; 32];

fn fingerprint(label: &str) -> Fingerprint {
    Fingerprint::digest(label.as_bytes())
}

fn environment() -> ArtifactEnvironment {
    ArtifactEnvironment {
        ty_revision: "2d16d8425179c3a235f8c57e72494728ff61a4f7".into(),
        checker_source_fingerprint: fingerprint("checker-with-dialect-and-conservative-analysis"),
        exporter_revision: "exporter-test-revision".into(),
        python_version: PythonVersion {
            major: 3,
            minor: 15,
        },
        python_platform: "linux-aarch64".into(),
        cpython_abi_fingerprint: fingerprint("cpython-abi"),
        normalized_project_policy: fingerprint("project-policy"),
        resolved_typechecker_configuration: fingerprint("checker-configuration"),
        import_search_path: fingerprint("search-path"),
        typeshed_fingerprint: fingerprint("typeshed"),
        installed_stub_fingerprint: fingerprint("installed-stubs"),
        installed_dependency_fingerprint: fingerprint("installed-dependencies"),
        analysis: ConservativeAnalysis::default(),
    }
}

fn signing_key() -> ArtifactSigningKey {
    ArtifactSigningKey::from_bytes(&SIGNING_SEED)
}

fn nominal_int() -> StaticType {
    StaticType::NominalBuiltin {
        builtin: BuiltinType::Int,
        allow_subclasses: true,
    }
}

fn range(source: &[u8], text: &str) -> SourceRange {
    let source = std::str::from_utf8(source).expect("fixture source is UTF-8");
    let start = source.find(text).expect("fixture text exists");
    SourceRange::new(start as u32, (start + text.len()) as u32)
}

fn definition(
    facts: &ModuleTypeFacts,
    name: &str,
    source_range: SourceRange,
    definition_kind: DefinitionKind,
) -> SourceIdentity {
    SourceIdentity {
        module: facts.module.clone(),
        lexical_qualname: name.into(),
        source_range,
        definition_kind,
    }
}

fn parameter(name: &str, value_type: StaticType) -> ParameterTypeFact {
    ParameterTypeFact {
        name: name.into(),
        kind: ParameterKind::PositionalOrKeyword,
        value_type,
        annotation_origin: AnnotationOrigin::Explicit,
        default: DefaultFact::Missing,
    }
}

fn signature(parameters: Vec<ParameterTypeFact>) -> CallableSignature {
    CallableSignature {
        parameters,
        return_type: nominal_int(),
        return_annotation_origin: AnnotationOrigin::Explicit,
        uncertainty: BTreeSet::new(),
    }
}

fn example_facts() -> ModuleTypeFacts {
    let mut facts = ModuleTypeFacts::new(
        "pkg.example",
        SOURCE,
        SourceDialect::SoacStrict,
        ResolvedStrictPolicy::default(),
    )
    .expect("fixture module");
    let class_start = range(SOURCE, "class Box:").start;
    let method_start = range(SOURCE, "def get").start;
    let read_start = range(SOURCE, "def read").start;
    let class_identity = definition(
        &facts,
        "Box",
        SourceRange::new(class_start, read_start),
        DefinitionKind::Class,
    );
    let class = ClassReference {
        definition: class_identity.clone(),
        source_digest: facts.source_digest,
    };
    let method_identity = definition(
        &facts,
        "Box.get",
        SourceRange::new(method_start, read_start),
        DefinitionKind::Function,
    );
    let read_identity = definition(
        &facts,
        "read",
        SourceRange::new(read_start, facts.source_size),
        DefinitionKind::Function,
    );
    let mut receiver = parameter("self", StaticType::NominalClass(class.clone()));
    receiver.annotation_origin = AnnotationOrigin::Inferred;
    let method_signature = signature(vec![receiver]);
    let read_signature = signature(vec![parameter(
        "box",
        StaticType::NominalClass(class.clone()),
    )]);
    facts.functions = vec![
        FunctionTypeFact {
            identity: method_identity.clone(),
            function_kind: FunctionKind::Synchronous,
            signature: method_signature.clone(),
            decorators: Vec::new(),
            uncertainty: BTreeSet::new(),
        },
        FunctionTypeFact {
            identity: read_identity.clone(),
            function_kind: FunctionKind::Synchronous,
            signature: read_signature.clone(),
            decorators: Vec::new(),
            uncertainty: BTreeSet::new(),
        },
    ];
    facts.classes.push(ClassTypeFact {
        identity: class_identity,
        bases: Vec::new(),
        metaclass: MetaclassFact::BuiltinType,
        decorators: Vec::new(),
        participation: ParticipationProposal::Candidate,
        dictionary: ClassDictionarySemantics::DictionaryBearing,
        instance_fields: vec![FieldTypeFact {
            name: "value".into(),
            declaring_class: class.clone(),
            value_type: nominal_int(),
            annotation_origin: AnnotationOrigin::Explicit,
            annotation_definition: Some(definition(
                &facts,
                "Box.value",
                range(SOURCE, "value: int"),
                DefinitionKind::Assignment,
            )),
            field_kind: FieldKind::InstanceField,
            read_policy: FieldReadPolicy::PythonAttribute,
            write_policy: FieldWritePolicy::DeclaredField,
            initialization: InitializationPolicy::MayBeAbsent,
            default: DefaultFact::Missing,
            descriptor: DescriptorFact::default(),
            uncertainty: BTreeSet::new(),
        }],
        methods: vec![MethodTypeFact {
            name: "get".into(),
            declaring_class: class.clone(),
            binding: MethodBinding::Instance,
            signature: method_signature.clone(),
            declared_final: false,
            override_policy: OverridePolicy::CompatibleSignatureRequired,
            implementation: Some(method_identity.clone()),
            generated: None,
            uncertainty: BTreeSet::new(),
        }],
        class_members: Vec::new(),
        inheritance: InheritanceFact {
            linearized_bases: Vec::new(),
            complete: true,
        },
        openness: ClassOpenness::OpenSubclassFamily,
        transform: None,
        uncertainty: BTreeSet::new(),
    });
    facts.global_bindings.push(GlobalBindingFact {
        name: "read".into(),
        mutability: GlobalMutability::FinalAfterSeal,
        value_type: StaticType::Callable(Box::new(read_signature)),
        definition: Some(read_identity.clone()),
        uncertainty: BTreeSet::new(),
    });
    facts.attribute_sites.push(AttributeSiteFact {
        identity: AttributeSiteIdentity {
            module: facts.module.clone(),
            source_digest: facts.source_digest,
            enclosing_function: method_identity,
            expression_range: range(SOURCE, "self.value"),
        },
        name: "value".into(),
        access: AttributeAccess::Read,
        receiver_type: StaticType::NominalClass(class.clone()),
        value_type: Some(nominal_int()),
        declaring_class: Some(class.clone()),
        uncertainty: BTreeSet::new(),
    });
    facts.call_sites.push(CallSiteFact {
        identity: CallSiteIdentity {
            module: facts.module.clone(),
            source_digest: facts.source_digest,
            enclosing_function: read_identity,
            expression_range: range(SOURCE, "box.get()"),
            expression_kind: CallExpressionKind::AttributeCall,
        },
        receiver: Some(ReceiverTypeFact {
            value_type: StaticType::NominalClass(class.clone()),
            uncertainty: BTreeSet::new(),
        }),
        attribute_name: Some("get".into()),
        candidate_targets: vec![CallableTargetFact::Method {
            class,
            name: "get".into(),
            implementation: facts.classes[0].methods[0].implementation.clone(),
        }],
        binding: CallBindingFact::BoundInstanceMethod,
        signature: method_signature,
        result_type: nominal_int(),
        uncertainty: CallUncertainty::OpenSubclassFamily,
    });
    facts
}

fn facts_with_nominal_binding() -> ModuleTypeFacts {
    let mut facts = example_facts();
    let class = ClassReference {
        definition: facts.classes[0].identity.clone(),
        source_digest: facts.source_digest,
    };
    facts.global_bindings.push(GlobalBindingFact {
        name: "Box".into(),
        mutability: GlobalMutability::FinalAfterSeal,
        value_type: StaticType::NominalBuiltin {
            builtin: BuiltinType::Type,
            allow_subclasses: true,
        },
        definition: Some(class.definition.clone()),
        uncertainty: BTreeSet::new(),
    });
    let annotation = range(SOURCE, "box: Box");
    facts.nominal_bindings.push(NominalBindingFact {
        owner: NominalBindingOwner::Function {
            function: facts.functions[1].identity.clone(),
            annotation: AnnotationTarget::Parameter { index: 0 },
        },
        expression_range: SourceRange::new(annotation.start + 5, annotation.end),
        name: "Box".into(),
        binding: class.definition.clone(),
        binding_scope: facts.module_body_identity(),
        class,
    });
    facts
}

#[test]
fn nominal_binding_schema_is_required_and_changes_authenticated_identity() {
    let facts = facts_with_nominal_binding();
    validate_module_facts(&facts, Some(SOURCE)).unwrap();
    let mut absent = serde_json::to_value(&facts).unwrap();
    absent.as_object_mut().unwrap().remove("nominal_bindings");
    assert!(serde_json::from_value::<ModuleTypeFacts>(absent).is_err());

    let present = Fixture::new(facts.clone());
    let mut unresolved = facts;
    unresolved.nominal_bindings.clear();
    let unresolved = Fixture::new(unresolved);
    assert_ne!(present.shard.digest(), unresolved.shard.digest());
    assert_ne!(present.manifest.generation, unresolved.manifest.generation);
    assert_eq!(present.verify_module().facts().nominal_bindings.len(), 1);
}

#[test]
fn nominal_binding_validation_checks_target_source_and_lexical_ownership() {
    for mutation in 0..10 {
        let mut facts = facts_with_nominal_binding();
        match mutation {
            0 => {
                facts.nominal_bindings[0].owner = NominalBindingOwner::Function {
                    function: facts.functions[1].identity.clone(),
                    annotation: AnnotationTarget::Parameter { index: 9 },
                }
            }
            1 => {
                facts.functions[1].signature.parameters[0].annotation_origin =
                    AnnotationOrigin::Inferred
            }
            2 => {
                facts.nominal_bindings[0].owner = NominalBindingOwner::Function {
                    function: facts.functions[1].identity.clone(),
                    annotation: AnnotationTarget::Return,
                }
            }
            3 => facts.nominal_bindings[0].name = "Different".into(),
            4 => facts.nominal_bindings[0].expression_range = range(SOURCE, "class Box"),
            5 => facts.nominal_bindings[0].class.source_digest = fingerprint("wrong class bytes"),
            6 => facts.nominal_bindings[0].binding_scope = facts.classes[0].identity.clone(),
            7 => facts.nominal_bindings[0].binding_scope = facts.functions[1].identity.clone(),
            8 => facts
                .nominal_bindings
                .push(facts.nominal_bindings[0].clone()),
            9 => {
                facts
                    .global_bindings
                    .iter_mut()
                    .find(|binding| binding.name == "Box")
                    .unwrap()
                    .definition = None
            }
            _ => unreachable!(),
        }
        assert!(
            validate_module_facts(&facts, Some(SOURCE)).is_err(),
            "mutation {mutation}"
        );
    }
}

const FIELD_NOMINAL_SOURCE: &[u8] = b"from __future__ import strict\n\nclass Target:\n    pass\n\nclass Holder:\n    value: Target\n    other: Target\n    class Nested:\n        nested: Target\n\nclass Child(Holder):\n    pass\n\ndef accept(item: Target) -> Target:\n    return item\n";

fn facts_with_field_nominal_bindings() -> ModuleTypeFacts {
    let source = FIELD_NOMINAL_SOURCE;
    let mut facts = ModuleTypeFacts::new(
        "pkg.fields",
        source,
        SourceDialect::SoacStrict,
        ResolvedStrictPolicy::default(),
    )
    .unwrap();
    let mut template = example_facts().classes.remove(0);
    template.instance_fields.clear();
    template.methods.clear();
    let classes = [
        ("Target", "class Target:\n    pass"),
        (
            "Holder",
            "class Holder:\n    value: Target\n    other: Target\n    class Nested:\n        nested: Target",
        ),
        ("Holder.Nested", "class Nested:\n        nested: Target"),
        ("Child", "class Child(Holder):\n    pass"),
    ];
    facts.classes = classes
        .into_iter()
        .map(|(name, text)| ClassTypeFact {
            identity: definition(&facts, name, range(source, text), DefinitionKind::Class),
            ..template.clone()
        })
        .collect();
    let reference = |index: usize| ClassReference {
        definition: facts.classes[index].identity.clone(),
        source_digest: facts.source_digest,
    };
    let target = reference(0);
    let holder = reference(1);
    let mut field_template = example_facts().classes.remove(0).instance_fields.remove(0);
    field_template.value_type = StaticType::NominalClass(target.clone());
    field_template.declaring_class = holder.clone();
    for name in ["value", "other"] {
        let annotation_definition = definition(
            &facts,
            &format!("Holder.{name}"),
            range(source, &format!("{name}: Target")),
            DefinitionKind::Assignment,
        );
        let field = FieldTypeFact {
            name: name.into(),
            annotation_definition: Some(annotation_definition.clone()),
            ..field_template.clone()
        };
        facts.nominal_bindings.push(NominalBindingFact {
            owner: NominalBindingOwner::Field {
                field: field.annotation_reference().unwrap(),
            },
            expression_range: SourceRange::new(
                annotation_definition.source_range.end - 6,
                annotation_definition.source_range.end,
            ),
            name: "Target".into(),
            class: target.clone(),
            binding: target.definition.clone(),
            binding_scope: facts.module_body_identity(),
        });
        facts.classes[1].instance_fields.push(field);
    }
    facts.classes[3]
        .bases
        .push(BaseReference::Class(holder.clone()));
    facts.classes[3]
        .inheritance
        .linearized_bases
        .push(BaseReference::Class(holder));
    facts.classes[3].instance_fields = facts.classes[1].instance_fields.clone();
    let function = definition(
        &facts,
        "accept",
        range(
            source,
            "def accept(item: Target) -> Target:\n    return item",
        ),
        DefinitionKind::Function,
    );
    facts.functions.push(FunctionTypeFact {
        identity: function.clone(),
        function_kind: FunctionKind::Synchronous,
        signature: CallableSignature {
            parameters: vec![parameter("item", StaticType::NominalClass(target.clone()))],
            return_type: StaticType::NominalClass(target.clone()),
            return_annotation_origin: AnnotationOrigin::Explicit,
            uncertainty: BTreeSet::new(),
        },
        decorators: Vec::new(),
        uncertainty: BTreeSet::new(),
    });
    let parameter = range(source, "item: Target");
    facts.nominal_bindings.push(NominalBindingFact {
        owner: NominalBindingOwner::Function {
            function,
            annotation: AnnotationTarget::Parameter { index: 0 },
        },
        expression_range: SourceRange::new(parameter.end - 6, parameter.end),
        name: "Target".into(),
        class: target.clone(),
        binding: target.definition.clone(),
        binding_scope: facts.module_body_identity(),
    });
    facts.global_bindings.push(GlobalBindingFact {
        name: "Target".into(),
        mutability: GlobalMutability::FinalAfterSeal,
        value_type: StaticType::NominalBuiltin {
            builtin: BuiltinType::Type,
            allow_subclasses: true,
        },
        definition: Some(target.definition),
        uncertainty: BTreeSet::new(),
    });
    facts
}

#[test]
fn field_nominal_owners_preserve_declarations_inheritance_and_annotation_isolation() {
    let facts = facts_with_field_nominal_bindings();
    validate_module_facts(&facts, Some(FIELD_NOMINAL_SOURCE)).unwrap();
    assert_eq!(facts.nominal_bindings.len(), 3);
    assert_ne!(
        facts.nominal_bindings[0].owner,
        facts.nominal_bindings[1].owner
    );
    assert_ne!(
        facts.nominal_bindings[0].owner,
        facts.nominal_bindings[2].owner
    );
    assert_eq!(
        facts.classes[1].instance_fields[0].annotation_reference(),
        facts.classes[3].instance_fields[0].annotation_reference(),
    );
    let fixture = Fixture::new(facts.clone());
    let verified = fixture.verify_module_source(FIELD_NOMINAL_SOURCE);
    assert_eq!(verified.facts(), &facts.canonicalized().unwrap());

    let mut unresolved = facts;
    unresolved
        .nominal_bindings
        .retain(|leaf| leaf.owner.as_function().is_some());
    for class in &mut unresolved.classes {
        for field in &mut class.instance_fields {
            field.annotation_definition = None;
        }
    }
    let unresolved = Fixture::new(unresolved);
    assert_ne!(fixture.shard.digest(), unresolved.shard.digest());
    assert_ne!(fixture.manifest.generation, unresolved.manifest.generation);
}

#[test]
fn field_nominal_validation_rejects_borrowed_annotations_and_wrong_contract_owners() {
    for mutation in 0..12 {
        let mut facts = facts_with_field_nominal_bindings();
        match mutation {
            0 => facts.nominal_bindings[0].owner = facts.nominal_bindings[1].owner.clone(),
            1 => facts.nominal_bindings[0].owner = facts.nominal_bindings[2].owner.clone(),
            2 => {
                let NominalBindingOwner::Field { field } = &mut facts.nominal_bindings[0].owner
                else {
                    unreachable!()
                };
                field.name = "other".into();
            }
            3 => {
                let NominalBindingOwner::Field { field } = &mut facts.nominal_bindings[0].owner
                else {
                    unreachable!()
                };
                field.declaring_class.definition = facts.classes[3].identity.clone();
            }
            4 => facts.classes[1].instance_fields[0].annotation_definition = None,
            5 => facts.classes[1].instance_fields[0].annotation_origin = AnnotationOrigin::Inferred,
            6 => facts.classes[1].instance_fields[0].value_type = nominal_int(),
            7 => facts
                .nominal_bindings
                .push(facts.nominal_bindings[0].clone()),
            8 => facts.nominal_bindings[0].binding_scope = facts.classes[1].identity.clone(),
            9 => {
                facts.classes[3].instance_fields[0]
                    .declaring_class
                    .definition = facts.classes[3].identity.clone();
            }
            10 => {
                let nested = definition(
                    &facts,
                    "Holder.Nested.nested",
                    range(FIELD_NOMINAL_SOURCE, "nested: Target"),
                    DefinitionKind::Assignment,
                );
                facts.classes[1].instance_fields[0].annotation_definition = Some(nested.clone());
                let NominalBindingOwner::Field { field } = &mut facts.nominal_bindings[0].owner
                else {
                    unreachable!()
                };
                field.annotation_definition = nested.clone();
                facts.nominal_bindings[0].expression_range =
                    SourceRange::new(nested.source_range.end - 6, nested.source_range.end);
            }
            11 => {
                let NominalBindingOwner::Field { field } = &mut facts.nominal_bindings[0].owner
                else {
                    unreachable!()
                };
                field.declaring_class.source_digest = fingerprint("foreign source bytes");
            }
            _ => unreachable!(),
        }
        assert!(
            validate_module_facts(&facts, Some(FIELD_NOMINAL_SOURCE)).is_err(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn ignored_field_annotations_do_not_retain_nominal_binding_authority() {
    let mut facts = facts_with_field_nominal_bindings();
    let field = facts.classes[1].instance_fields[0]
        .annotation_definition
        .clone()
        .unwrap();
    facts
        .diagnostics
        .push(ignored_region(&facts, DiagnosticScope::Definition(field)));
    let fixture = Fixture::new(facts);
    let verified = fixture.verify_module_source(FIELD_NOMINAL_SOURCE);
    let facts = verified.facts();
    assert!(
        facts
            .nominal_bindings
            .iter()
            .all(|leaf| leaf.owner.as_function().is_some())
    );
    assert_eq!(facts.nominal_bindings.len(), 1);
    for name in ["Holder", "Child"] {
        let class = facts
            .classes
            .iter()
            .find(|class| class.identity.lexical_qualname == name)
            .unwrap();
        assert!(
            class
                .instance_fields
                .iter()
                .all(|field| field.value_type == StaticType::Unknown)
        );
    }
}

#[test]
fn static_dynamic_method_policy_requires_exact_local_ownership() {
    let mut facts = example_facts();
    let method = facts.functions[0].identity.clone();
    let standalone = facts.functions[1].identity.clone();
    assert_eq!(facts.source_class_owner(&method), Some(&facts.classes[0]));
    assert!(facts.source_class_owner(&standalone).is_none());
    assert!(!facts.function_has_statically_dynamic_class_owner(&method));
    facts.classes[0].participation = ParticipationProposal::Dynamic(BTreeSet::from([
        DynamicClassReason::NonParticipatingMetaclass,
    ]));
    assert!(facts.function_has_statically_dynamic_class_owner(&method));
    assert!(!facts.function_has_statically_dynamic_class_owner(&standalone));

    // Assigning an independent function into a dynamic class does not change
    // the source function's contract, even if its member name looks local.
    facts.classes[0].methods[0].implementation = Some(standalone.clone());
    assert!(facts.source_class_owner(&standalone).is_none());
    assert!(!facts.function_has_statically_dynamic_class_owner(&standalone));
    facts.classes[0].methods[0].implementation = Some(method.clone());

    let mut foreign = method.clone();
    foreign.module.module_name = "different_module".into();
    facts.classes[0].methods[0].implementation = Some(foreign.clone());
    assert!(!facts.function_has_statically_dynamic_class_owner(&foreign));
    facts.classes[0].methods[0].implementation = Some(method.clone());

    let mut same_name = method.clone();
    same_name.source_range.start += 1;
    assert!(!facts.function_has_statically_dynamic_class_owner(&same_name));
    facts.classes[0].identity.source_range.end = method.source_range.start;
    assert!(!facts.function_has_statically_dynamic_class_owner(&method));
}

#[test]
fn static_dynamic_method_policy_retains_overwritten_lexical_definitions() {
    let mut facts = example_facts();
    let method = facts.functions[0].identity.clone();
    let standalone = facts.functions[1].identity.clone();
    facts.classes[0].participation =
        ParticipationProposal::Dynamic(BTreeSet::from([DynamicClassReason::UnknownDecorator]));
    facts.classes[0].methods.clear();
    facts.classes[0].instance_fields.clear();
    facts.classes[0].class_members.clear();
    assert_eq!(facts.source_class_owner(&method), Some(&facts.classes[0]));
    assert!(facts.function_has_statically_dynamic_class_owner(&method));

    let descriptor = DescriptorFact {
        kind: DescriptorKind::Property,
        getter: Some(standalone.clone()),
        ..Default::default()
    };
    facts.classes[0].class_members.push(ClassMemberFact {
        name: "property".into(),
        kind: ClassMemberKind::Descriptor,
        value_type: StaticType::Unknown,
        definition: Some(standalone.clone()),
        descriptor,
        uncertainty: BTreeSet::new(),
    });
    assert!(facts.function_has_statically_dynamic_class_owner(&method));
    assert!(facts.source_class_owner(&standalone).is_none());
    assert!(!facts.function_has_statically_dynamic_class_owner(&standalone));
}

#[test]
fn static_dynamic_method_policy_does_not_cross_nested_lexical_owners() {
    const NESTED: &[u8] = b"from __future__ import strict\nclass Outer:\n    class Inner:\n        def get(self):\n            return 1\n    borrowed = property(Inner.get)\n    def factory(self):\n        def nested():\n            return 2\n        return nested\n";
    let mut facts = ModuleTypeFacts::new(
        "nested_owners",
        NESTED,
        SourceDialect::SoacStrict,
        ResolvedStrictPolicy::default(),
    )
    .unwrap();
    let getter = definition(
        &facts,
        "Outer.Inner.get",
        range(NESTED, "def get(self):\n            return 1"),
        DefinitionKind::Function,
    );
    let factory = definition(
        &facts,
        "Outer.factory",
        SourceRange::new(range(NESTED, "def factory").start, facts.source_size),
        DefinitionKind::Function,
    );
    let nested = definition(
        &facts,
        "Outer.factory.<locals>.nested",
        range(NESTED, "def nested():\n            return 2"),
        DefinitionKind::Function,
    );
    let mut outer = example_facts().classes.remove(0);
    outer.identity = definition(
        &facts,
        "Outer",
        SourceRange::new(range(NESTED, "class Outer").start, facts.source_size),
        DefinitionKind::Class,
    );
    outer.participation = ParticipationProposal::Dynamic(BTreeSet::from([
        DynamicClassReason::NonParticipatingMetaclass,
    ]));
    outer.instance_fields.clear();
    outer.methods[0].declaring_class = ClassReference {
        definition: outer.identity.clone(),
        source_digest: facts.source_digest,
    };
    outer.methods[0].name = "factory".into();
    outer.methods[0].implementation = Some(factory.clone());
    outer.methods[0].signature = signature(Vec::new());
    outer.class_members = [&getter, &nested]
        .into_iter()
        .enumerate()
        .map(|(index, component)| ClassMemberFact {
            name: format!("borrowed_{index}"),
            kind: ClassMemberKind::Descriptor,
            value_type: StaticType::Unknown,
            definition: None,
            descriptor: DescriptorFact {
                kind: DescriptorKind::Property,
                getter: Some(component.clone()),
                ..Default::default()
            },
            uncertainty: BTreeSet::new(),
        })
        .collect();
    let mut inner = outer.clone();
    inner.identity = definition(
        &facts,
        "Outer.Inner",
        SourceRange::new(range(NESTED, "class Inner").start, getter.source_range.end),
        DefinitionKind::Class,
    );
    inner.participation = ParticipationProposal::Candidate;
    inner.methods[0].name = "get".into();
    inner.methods[0].declaring_class.definition = inner.identity.clone();
    inner.methods[0].implementation = Some(getter.clone());
    inner.class_members.clear();
    facts.classes = vec![outer, inner];
    facts.functions = [&getter, &factory, &nested]
        .into_iter()
        .map(|identity| FunctionTypeFact {
            identity: identity.clone(),
            function_kind: FunctionKind::Synchronous,
            signature: signature(Vec::new()),
            decorators: Vec::new(),
            uncertainty: BTreeSet::new(),
        })
        .collect();
    assert!(!facts.function_has_statically_dynamic_class_owner(&getter));
    assert!(!facts.function_has_statically_dynamic_class_owner(&nested));
    assert!(facts.function_has_statically_dynamic_class_owner(&factory));
    assert_eq!(facts.source_class_owner(&getter), Some(&facts.classes[1]));
    assert_eq!(facts.source_class_owner(&factory), Some(&facts.classes[0]));
    assert!(facts.source_class_owner(&nested).is_none());
}

struct Fixture {
    facts: ModuleTypeFacts,
    shard: EncodedModuleShard,
    manifest: TypeArtifactManifest,
    signed: Vec<u8>,
    expected: ArtifactExpectations,
}

impl Fixture {
    fn new(facts: ModuleTypeFacts) -> Self {
        let shard = encode_module_shard(&facts).expect("encode fixture shard");
        let manifest = TypeArtifactManifest::new(
            environment(),
            vec![ModuleArtifactIndex::from_shard(&shard).expect("module index")],
        )
        .expect("fixture manifest");
        let signed = sign_manifest(&manifest, &signing_key()).expect("sign fixture manifest");
        let expected = ArtifactExpectations {
            generation: manifest.generation,
            environment: environment(),
        };
        Self {
            facts,
            shard,
            manifest,
            signed,
            expected,
        }
    }

    fn verify(&self) -> VerifiedTypeArtifactManifest {
        verify_manifest(&self.signed, &signing_key().trust_anchor(), &self.expected)
            .expect("verify fixture manifest")
    }

    fn verify_module(&self) -> VerifiedModuleTypeFacts {
        self.verify_module_source(SOURCE)
    }

    fn verify_module_source(&self, source: &[u8]) -> VerifiedModuleTypeFacts {
        self.verify()
            .verify_module(
                &self.facts.module.module_name,
                source,
                &self.facts.language_policy,
                &self.facts.consumed_dependencies,
                self.shard.bytes(),
            )
            .expect("verify fixture module")
    }
}

/// Independent test-side producer for signed-but-invalid manifests. This
/// deliberately bypasses the public signer's schema validation to exercise
/// the loader, not just the producer, at the real cryptographic boundary.
fn sign_unvalidated(manifest: &TypeArtifactManifest) -> Vec<u8> {
    let mut payload = crate::artifact::SIGNATURE_DOMAIN.to_vec();
    payload.extend_from_slice(&canonical_bytes(manifest).expect("test manifest payload"));
    let signature = ed25519_dalek::SigningKey::from_bytes(&SIGNING_SEED).sign(&payload);
    let signature = crate::identity::encode_hex(&signature.to_bytes());
    canonical_bytes(&serde_json::json!({ "manifest": manifest, "signature": signature }))
        .expect("test signed envelope")
}

#[test]
fn authenticated_proposal_is_bound_to_source_policy_and_generation() {
    let fixture = Fixture::new(example_facts());
    let verified = fixture.verify_module();
    assert_eq!(
        verified.facts(),
        &fixture.facts.canonicalized().expect("normalize")
    );
    assert_eq!(verified.generation(), fixture.manifest.generation);
    assert_eq!(verified.shard_digest(), fixture.shard.digest());
    assert_eq!(
        verified.facts().classes[0].dictionary,
        ClassDictionarySemantics::DictionaryBearing
    );
    assert_eq!(
        verified.facts().classes[0].instance_fields[0].initialization,
        InitializationPolicy::MayBeAbsent
    );
    assert_eq!(
        verified.facts().call_sites[0].uncertainty,
        CallUncertainty::OpenSubclassFamily
    );
}

#[test]
fn module_shards_and_source_identities_are_deterministic() {
    let first = example_facts();
    let mut second = example_facts();
    second.functions.reverse();
    second.global_bindings.reverse();
    let first = encode_module_shard(&first).expect("first shard");
    let second = encode_module_shard(&second).expect("second shard");
    assert_eq!(first.digest(), second.digest());
    assert_eq!(first.bytes(), second.bytes());
    assert_eq!(first.facts().functions, second.facts().functions);
    let first_manifest = TypeArtifactManifest::new(
        environment(),
        vec![ModuleArtifactIndex::from_shard(&first).expect("index")],
    )
    .expect("manifest");
    let second_manifest = TypeArtifactManifest::new(
        environment(),
        vec![ModuleArtifactIndex::from_shard(&second).expect("index")],
    )
    .expect("manifest");
    assert_eq!(first_manifest.generation, second_manifest.generation);
    assert_eq!(
        sign_manifest(&first_manifest, &signing_key()).expect("sign"),
        sign_manifest(&second_manifest, &signing_key()).expect("sign")
    );
}

#[test]
fn union_normalization_preserves_uncertainty_and_subclass_acceptance() {
    let optional = StaticType::Optional(Box::new(nominal_int()))
        .normalized()
        .expect("optional");
    let union = StaticType::Union(vec![
        StaticType::Literal(LiteralValue::None),
        nominal_int(),
        nominal_int(),
    ])
    .normalized()
    .expect("union");
    assert_eq!(optional, union);
    let uncertain = StaticType::Union(vec![
        StaticType::Unknown,
        StaticType::Any,
        StaticType::Todo,
        StaticType::Divergent,
        StaticType::Union(vec![nominal_int(), StaticType::Unknown]),
    ])
    .normalized()
    .expect("uncertain union");
    let StaticType::Union(elements) = &uncertain else {
        panic!("expected union")
    };
    assert_eq!(elements.len(), 5);
    assert!(elements.contains(&nominal_int()));
    assert!(uncertain.contains_uncertainty());
    assert!(!uncertain.has_supported_value_shape());
    assert_ne!(nominal_int(), StaticType::ExactBuiltin(BuiltinType::Int));
    assert!(matches!(
        StaticType::Union(Vec::new()).normalized(),
        Err(ContractError::InvalidType(_))
    ));
}

#[test]
fn changing_authority_or_recomputing_unkeyed_digests_does_not_authenticate() {
    let fixture = Fixture::new(example_facts());
    let other_key = ArtifactSigningKey::from_bytes(&[37; 32]);
    assert!(matches!(
        verify_manifest(
            &fixture.signed,
            &other_key.trust_anchor(),
            &fixture.expected
        ),
        Err(ContractError::UntrustedSignature)
    ));

    let mut changed = example_facts();
    changed.classes[0].instance_fields[0].value_type = StaticType::ExactBuiltin(BuiltinType::Str);
    let changed = encode_module_shard(&changed).expect("changed valid proposal");
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&fixture.signed).expect("envelope");
    envelope["manifest"]["modules"][0]["shard_digest"] =
        serde_json::to_value(changed.digest()).expect("digest");
    let forged = canonical_bytes(&envelope).expect("forged envelope");
    assert!(matches!(
        verify_manifest(&forged, &signing_key().trust_anchor(), &fixture.expected),
        Err(ContractError::UntrustedSignature)
    ));
    envelope["trust_anchor"] =
        serde_json::to_value(other_key.trust_anchor().to_bytes()).expect("key");
    assert!(matches!(
        verify_manifest(
            &canonical_bytes(&envelope).expect("envelope"),
            &signing_key().trust_anchor(),
            &fixture.expected
        ),
        Err(ContractError::Encoding(_))
    ));
}

#[test]
fn signed_generations_cannot_be_replayed_or_reused_for_changed_contents() {
    let fixture = Fixture::new(example_facts());
    let mut expected = fixture.expected.clone();
    expected.generation = ArtifactGenerationId::new(fingerprint("another deployment"));
    assert!(matches!(
        verify_manifest(&fixture.signed, &signing_key().trust_anchor(), &expected),
        Err(ContractError::GenerationMismatch)
    ));

    let mut changed = fixture.manifest.clone();
    changed.modules[0].source_digest = fingerprint("changed source");
    assert!(matches!(
        verify_manifest(
            &sign_unvalidated(&changed),
            &signing_key().trust_anchor(),
            &fixture.expected
        ),
        Err(ContractError::GenerationMismatch)
    ));
}

#[test]
fn loader_rejects_every_incompatible_schema_contract_and_dialect_version() {
    let fixture = Fixture::new(example_facts());
    for which in 0..3 {
        let mut manifest = fixture.manifest.clone();
        match which {
            0 => manifest.versions.schema_version += 1,
            1 => manifest.versions.strict_contract_version += 1,
            _ => manifest.versions.dialect_version += 1,
        }
        assert!(matches!(
            verify_manifest(
                &sign_unvalidated(&manifest),
                &signing_key().trust_anchor(),
                &fixture.expected
            ),
            Err(ContractError::VersionMismatch { .. })
        ));
    }
}

#[test]
fn required_provenance_schema_and_signature_versions_reject_legacy_authority() {
    let fixture = Fixture::new(example_facts());
    for version in [1, 2, 3, 4, 5] {
        let mut legacy = fixture.facts.clone();
        legacy.schema_version = version;
        assert!(matches!(
            validate_module_facts(&legacy, Some(SOURCE)),
            Err(ContractError::VersionMismatch {
                kind: "module shard schema",
                expected: ARTIFACT_SCHEMA_VERSION,
                found,
            }) if found == version
        ));
    }
    let mut legacy_manifest = fixture.manifest.clone();
    legacy_manifest.versions.schema_version = 1;
    assert!(matches!(
        verify_manifest(
            &sign_unvalidated(&legacy_manifest),
            &signing_key().trust_anchor(),
            &fixture.expected
        ),
        Err(ContractError::VersionMismatch { .. })
    ));
    // Reusing the old signing domain cannot authorize even a new-schema
    // payload with all of its unkeyed content hashes recomputed.
    let mut legacy_payload = b"SOAC-TYPE-CONTRACT-MANIFEST\0v5\0".to_vec();
    legacy_payload.extend_from_slice(&canonical_bytes(&fixture.manifest).unwrap());
    let signature = ed25519_dalek::SigningKey::from_bytes(&SIGNING_SEED).sign(&legacy_payload);
    let old_signed = canonical_bytes(&serde_json::json!({
        "manifest": fixture.manifest,
        "signature": crate::identity::encode_hex(&signature.to_bytes()),
    }))
    .unwrap();
    assert!(matches!(
        verify_manifest(
            &old_signed,
            &signing_key().trust_anchor(),
            &fixture.expected
        ),
        Err(ContractError::UntrustedSignature)
    ));
}

#[test]
fn signed_shards_require_field_annotation_provenance_without_implicit_defaults() {
    let fixture = Fixture::new(example_facts());
    for field in ["annotation_origin", "annotation_definition"] {
        let mut value = serde_json::to_value(fixture.facts.canonicalized().unwrap()).unwrap();
        value["classes"][0]["instance_fields"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        let raw = canonical_bytes(&value).unwrap();
        let mut index = ModuleArtifactIndex::from_shard(&fixture.shard).unwrap();
        index.shard_digest = Fingerprint::digest(&raw);
        let manifest = TypeArtifactManifest::new(environment(), vec![index]).unwrap();
        let verified = verify_manifest(
            &sign_manifest(&manifest, &signing_key()).unwrap(),
            &signing_key().trust_anchor(),
            &ArtifactExpectations {
                generation: manifest.generation,
                environment: environment(),
            },
        )
        .unwrap();
        assert!(
            matches!(
                verified.verify_module(
                    "pkg.example",
                    SOURCE,
                    &fixture.facts.language_policy,
                    &[],
                    &raw
                ),
                Err(ContractError::Encoding(_))
            ),
            "missing {field}"
        );
    }
}

#[test]
fn loader_checks_resolved_environment_inputs_independently() {
    let fixture = Fixture::new(example_facts());
    let updates: &[fn(&mut ArtifactEnvironment)] = &[
        |value| value.ty_revision.push_str("-changed"),
        |value| value.checker_source_fingerprint = fingerprint("changed checker patches"),
        |value| value.exporter_revision.push_str("-changed"),
        |value| value.python_version.minor = 14,
        |value| value.python_platform = "win32".into(),
        |value| value.cpython_abi_fingerprint = fingerprint("changed ABI"),
        |value| value.normalized_project_policy = fingerprint("changed project policy"),
        |value| value.resolved_typechecker_configuration = fingerprint("changed per-file override"),
        |value| value.import_search_path = fingerprint("changed search path"),
        |value| value.typeshed_fingerprint = fingerprint("changed typeshed"),
        |value| value.installed_stub_fingerprint = fingerprint("changed installed stub"),
        |value| value.installed_dependency_fingerprint = fingerprint("changed dependency version"),
    ];
    for update in updates {
        let mut expected = fixture.expected.clone();
        update(&mut expected.environment);
        assert!(matches!(
            verify_manifest(&fixture.signed, &signing_key().trust_anchor(), &expected),
            Err(ContractError::EnvironmentMismatch(_))
        ));
    }
}

#[test]
fn unsafe_checker_analysis_settings_fail_closed_even_with_a_valid_signature() {
    let fixture = Fixture::new(example_facts());
    for equality in [true, false] {
        let mut manifest = fixture.manifest.clone();
        if equality {
            manifest.environment.analysis.strict_equality_semantics = false;
        } else {
            manifest.environment.analysis.strict_generic_narrowing = false;
        }
        assert!(matches!(
            verify_manifest(
                &sign_unvalidated(&manifest),
                &signing_key().trust_anchor(),
                &fixture.expected
            ),
            Err(ContractError::InvalidPolicy(_))
        ));
    }
}

#[test]
fn source_policy_and_legacy_identity_mismatches_fail_before_facts_are_returned() {
    let fixture = Fixture::new(example_facts());
    let manifest = fixture.verify();
    let mut changed_source = SOURCE.to_vec();
    changed_source.push(b'\n');
    assert!(matches!(
        manifest.verify_module(
            "pkg.example",
            &changed_source,
            &fixture.facts.language_policy,
            &[],
            fixture.shard.bytes()
        ),
        Err(ContractError::SourceMismatch(_))
    ));
    let mut policy = fixture.facts.language_policy.clone();
    policy.checked_fields = CheckedFieldPolicy::SupportedAnnotations;
    assert!(matches!(
        manifest.verify_module("pkg.example", SOURCE, &policy, &[], fixture.shard.bytes()),
        Err(ContractError::PolicyMismatch(_))
    ));
    let mut wrong_hash = fixture.manifest.clone();
    wrong_hash.modules[0].module.source_hash ^= 1;
    let wrong_hash = TypeArtifactManifest::new(wrong_hash.environment, wrong_hash.modules)
        .expect("different manifest");
    let expected = ArtifactExpectations {
        generation: wrong_hash.generation,
        environment: environment(),
    };
    let manifest = verify_manifest(
        &sign_manifest(&wrong_hash, &signing_key()).expect("sign"),
        &signing_key().trust_anchor(),
        &expected,
    )
    .expect("verify manifest");
    assert!(matches!(
        manifest.verify_module(
            "pkg.example",
            SOURCE,
            &fixture.facts.language_policy,
            &[],
            fixture.shard.bytes()
        ),
        Err(ContractError::SourceMismatch(_))
    ));
}

#[test]
fn field_binding_selection_depends_only_on_selected_storage_writes() {
    let facts = example_facts();
    let mut class = facts.classes[0].clone();
    let class_reference = class.instance_fields[0].declaring_class.clone();
    class.instance_fields[0].value_type = StaticType::NominalClass(class_reference);
    let mut policy = ResolvedStrictPolicy::default();
    assert!(class.required_field_bindings(&policy).is_empty());

    policy.checked_fields = CheckedFieldPolicy::SupportedAnnotations;
    assert_eq!(
        class.required_field_bindings(&policy),
        vec![&class.instance_fields[0]]
    );
    for origin in [AnnotationOrigin::Absent, AnnotationOrigin::Inferred] {
        class.instance_fields[0].annotation_origin = origin;
        assert!(class.required_field_bindings(&policy).is_empty());
    }
    class.instance_fields[0].annotation_origin = AnnotationOrigin::Explicit;
    class.instance_fields[0].descriptor.kind = DescriptorKind::Property;
    assert!(class.required_field_bindings(&policy).is_empty());
    class.instance_fields[0].descriptor.kind = DescriptorKind::None;

    class.transform = Some(ClassTransformFact {
        kind: TransformKind::StdlibDataclass,
        provenance: None,
        dataclass_options: Some(DataclassOptions::default()),
        generated_methods: BTreeSet::from(["__init__".into()]),
    });
    for kind in [FieldKind::InitOnly, FieldKind::ClassVariable] {
        class.instance_fields[0].field_kind = kind;
        assert!(
            class.required_field_bindings(&policy).is_empty(),
            "constructor pseudo-fields are not protected storage"
        );
    }
    class.instance_fields[0].field_kind = FieldKind::InstanceField;
    policy.checked_fields = CheckedFieldPolicy::Disabled;
    assert!(
        class.required_field_bindings(&policy).is_empty(),
        "a dataclass constructor must not invent a runtime field requirement"
    );
    let transform = class.transform.as_mut().unwrap();
    transform.dataclass_options.as_mut().unwrap().init = false;
    transform.generated_methods.clear();
    assert!(class.required_field_bindings(&policy).is_empty());

    policy.checked_fields = CheckedFieldPolicy::SupportedAnnotations;
    assert_eq!(
        class.required_field_bindings(&policy),
        vec![&class.instance_fields[0]],
        "checked storage does not depend on having a generated initializer"
    );
    class.instance_fields[0]
        .declaring_class
        .definition
        .lexical_qualname = "OriginalBase".into();
    assert!(
        class.required_field_bindings(&policy).is_empty(),
        "inherited fields keep their actual declaring owner"
    );
}

#[test]
fn removed_function_type_policies_cannot_be_silently_reinterpreted() {
    for (key, value) in [
        ("checked_parameters", "supported_annotations"),
        ("checked_returns", "disabled"),
        ("parameter_failure", "type_error"),
        ("return_failure", "type_error"),
    ] {
        let mut encoded = serde_json::to_value(ResolvedStrictPolicy::default()).unwrap();
        encoded
            .as_object_mut()
            .unwrap()
            .insert(key.into(), value.into());
        assert!(
            serde_json::from_value::<ResolvedStrictPolicy>(encoded).is_err(),
            "retired policy key must be rejected: {key}"
        );
    }
}

#[test]
fn changed_policy_changes_content_generation_and_cache_identity_for_identical_source() {
    let first = Fixture::new(example_facts());
    let mut changed = example_facts();
    changed.language_policy.checked_fields = CheckedFieldPolicy::SupportedAnnotations;
    let second = Fixture::new(changed);
    assert_eq!(first.facts.module, second.facts.module);
    assert_eq!(first.facts.source_digest, second.facts.source_digest);
    assert_ne!(first.shard.digest(), second.shard.digest());
    assert_ne!(first.manifest.generation, second.manifest.generation);
    assert_ne!(
        first.verify_module().cache_identity(),
        second.verify_module().cache_identity()
    );
}

#[test]
fn changing_only_field_annotation_origin_invalidates_shard_generation_and_cache_identity() {
    let explicit = Fixture::new(example_facts());
    let mut inferred = example_facts();
    inferred.classes[0].instance_fields[0].annotation_origin = AnnotationOrigin::Inferred;
    let inferred = Fixture::new(inferred);
    assert_eq!(explicit.facts.source_digest, inferred.facts.source_digest);
    assert_eq!(
        explicit.facts.classes[0].instance_fields[0].value_type,
        inferred.facts.classes[0].instance_fields[0].value_type,
    );
    assert_ne!(explicit.shard.digest(), inferred.shard.digest());
    assert_ne!(explicit.manifest.generation, inferred.manifest.generation);
    assert_ne!(
        explicit.verify_module().cache_identity(),
        inferred.verify_module().cache_identity()
    );
}

fn dependency(module_name: &str, source: &[u8]) -> DependencyFingerprint {
    DependencyFingerprint {
        module: ModuleContentId::new(module_name, legacy_source_hash(source)),
        source_digest: Fingerprint::digest(source),
        source_size: source.len() as u32,
        import_resolution: fingerprint("resolved source path"),
        effective_configuration: fingerprint("resolved dependency configuration"),
        strict_policy: None,
        type_contract: None,
    }
}

#[test]
fn consumed_dependency_source_resolution_and_contract_mismatches_fail_closed() {
    let mut facts = example_facts();
    let external = dependency("pkg.base", b"class Base:\n    pass\n");
    let class = ClassReference {
        definition: SourceIdentity {
            module: external.module.clone(),
            lexical_qualname: "Base".into(),
            source_range: SourceRange::new(0, external.source_size),
            definition_kind: DefinitionKind::Class,
        },
        source_digest: external.source_digest,
    };
    facts.classes[0]
        .bases
        .push(BaseReference::Class(class.clone()));
    facts.classes[0]
        .inheritance
        .linearized_bases
        .push(BaseReference::Class(class));
    facts.consumed_dependencies.push(external.clone());
    let fixture = Fixture::new(facts);
    fixture.verify_module();
    let mutations: &[fn(&mut DependencyFingerprint)] = &[
        |value| value.module.source_hash ^= 1,
        |value| value.source_digest = fingerprint("changed dependency source"),
        |value| value.source_size += 1,
        |value| value.import_resolution = fingerprint("changed resolver target"),
        |value| value.effective_configuration = fingerprint("changed per-file dependency config"),
        |value| value.strict_policy = Some(fingerprint("different dependency policy")),
        |value| value.type_contract = Some(fingerprint("different dependency contract")),
    ];
    for mutation in mutations {
        let mut changed = external.clone();
        mutation(&mut changed);
        assert!(matches!(
            fixture.verify().verify_module(
                "pkg.example",
                SOURCE,
                &fixture.facts.language_policy,
                &[changed],
                fixture.shard.bytes()
            ),
            Err(ContractError::DependencyMismatch(_))
        ));
    }
    assert!(matches!(
        fixture.verify().verify_module(
            "pkg.example",
            SOURCE,
            &fixture.facts.language_policy,
            &[],
            fixture.shard.bytes()
        ),
        Err(ContractError::DependencyMismatch(_))
    ));
}

#[test]
fn complete_generation_rejects_missing_or_mixed_shards_and_reuses_unchanged_shards() {
    let stable = encode_module_shard(&example_facts()).expect("stable shard");
    let other_source = b"from __future__ import strict\nanswer = 1\n";
    let other = ModuleTypeFacts::new(
        "pkg.other",
        other_source,
        SourceDialect::SoacStrict,
        ResolvedStrictPolicy::default(),
    )
    .expect("other facts");
    let other = encode_module_shard(&other).expect("other shard");
    let indices = vec![
        ModuleArtifactIndex::from_shard(&other).expect("index"),
        ModuleArtifactIndex::from_shard(&stable).expect("index"),
    ];
    let manifest = TypeArtifactManifest::new(environment(), indices).expect("manifest");
    let expected = ArtifactExpectations {
        generation: manifest.generation,
        environment: environment(),
    };
    let signed = sign_manifest(&manifest, &signing_key()).expect("signed manifest");
    let verified = verify_manifest(&signed, &signing_key().trust_anchor(), &expected)
        .expect("verified manifest");
    assert!(matches!(
        verify_complete_generation(verified.clone(), |digest| {
            if digest == stable.digest() {
                Ok(stable.bytes().to_vec())
            } else {
                Err(ContractError::MissingShard(digest.to_hex()))
            }
        }),
        Err(ContractError::MissingShard(_))
    ));
    assert!(matches!(
        verify_complete_generation(verified.clone(), |_| Ok(stable.bytes().to_vec())),
        Err(ContractError::ShardMismatch(_))
    ));
    let complete = verify_complete_generation(verified, |digest| {
        if digest == stable.digest() {
            Ok(stable.bytes().to_vec())
        } else {
            Ok(other.bytes().to_vec())
        }
    })
    .expect("complete snapshot");
    assert_eq!(
        complete.manifest().manifest().generation,
        manifest.generation
    );

    let changed_source = b"from __future__ import strict\nanswer = 2\n";
    let changed = ModuleTypeFacts::new(
        "pkg.other",
        changed_source,
        SourceDialect::SoacStrict,
        ResolvedStrictPolicy::default(),
    )
    .expect("changed facts");
    let changed = encode_module_shard(&changed).expect("changed shard");
    let next = TypeArtifactManifest::new(
        environment(),
        vec![
            ModuleArtifactIndex::from_shard(&stable).expect("stable index"),
            ModuleArtifactIndex::from_shard(&changed).expect("changed index"),
        ],
    )
    .expect("next generation");
    assert_ne!(next.generation, manifest.generation);
    assert_eq!(
        next.modules[0].shard_digest,
        manifest.modules[0].shard_digest
    );
    assert_ne!(
        next.modules[1].shard_digest,
        manifest.modules[1].shard_digest
    );
}

#[test]
fn internal_dependency_versions_cannot_be_mixed_with_another_generation() {
    let fixture = Fixture::new(example_facts());
    let mut other = ModuleTypeFacts::new(
        "pkg.other",
        b"from __future__ import strict\n",
        SourceDialect::SoacStrict,
        ResolvedStrictPolicy::default(),
    )
    .expect("other");
    let mut stale = dependency("pkg.example", SOURCE);
    stale.source_digest = fingerprint("older producer source");
    other.consumed_dependencies.push(stale);
    let other = encode_module_shard(&other).expect("consumer shard");
    assert!(matches!(
        TypeArtifactManifest::new(
            environment(),
            vec![
                ModuleArtifactIndex::from_shard(&fixture.shard).expect("producer index"),
                ModuleArtifactIndex::from_shard(&other).expect("consumer index")
            ]
        ),
        Err(ContractError::DependencyMismatch(_))
    ));
}

#[test]
fn invalid_ranges_local_references_and_class_digests_are_rejected() {
    let mut range = example_facts();
    range.classes[0].identity.source_range.end = range.source_size + 1;
    assert!(matches!(
        encode_module_shard(&range),
        Err(ContractError::InvalidSourceIdentity(_))
    ));
    let mut missing = example_facts();
    missing.functions.remove(0);
    assert!(matches!(
        encode_module_shard(&missing),
        Err(ContractError::InvalidSourceIdentity(_))
    ));
    let mut wrong_digest = example_facts();
    wrong_digest.classes[0].instance_fields[0]
        .declaring_class
        .source_digest = fingerprint("wrong class source");
    assert!(matches!(
        encode_module_shard(&wrong_digest),
        Err(ContractError::SourceMismatch(_))
    ));
    let mut duplicate = example_facts();
    duplicate.classes.push(duplicate.classes[0].clone());
    assert!(matches!(
        encode_module_shard(&duplicate),
        Err(ContractError::InvalidStructure(_))
    ));
    let mut outside = example_facts();
    outside.call_sites[0].identity.expression_range = SourceRange::new(0, 1);
    assert!(matches!(
        encode_module_shard(&outside),
        Err(ContractError::InvalidStructure(_))
    ));
}

#[test]
fn source_ranges_must_follow_utf8_byte_boundaries() {
    let source = "from __future__ import strict\ndef café():\n    return 1\n".as_bytes();
    let mut facts = ModuleTypeFacts::new(
        "pkg.unicode",
        source,
        SourceDialect::SoacStrict,
        ResolvedStrictPolicy::default(),
    )
    .expect("facts");
    let invalid_start = range(source, "é").start + 1;
    facts.functions.push(FunctionTypeFact {
        identity: definition(
            &facts,
            "café",
            SourceRange::new(invalid_start, facts.source_size),
            DefinitionKind::Function,
        ),
        function_kind: FunctionKind::Synchronous,
        signature: signature(Vec::new()),
        decorators: Vec::new(),
        uncertainty: BTreeSet::new(),
    });
    let shard =
        encode_module_shard(&facts).expect("unsigned proposal cannot inspect absent source bytes");
    let manifest = TypeArtifactManifest::new(
        environment(),
        vec![ModuleArtifactIndex::from_shard(&shard).expect("index")],
    )
    .expect("manifest");
    let expected = ArtifactExpectations {
        generation: manifest.generation,
        environment: environment(),
    };
    let verified = verify_manifest(
        &sign_manifest(&manifest, &signing_key()).expect("sign"),
        &signing_key().trust_anchor(),
        &expected,
    )
    .expect("manifest");
    assert!(matches!(
        verified.verify_module(
            "pkg.unicode",
            source,
            &facts.language_policy,
            &[],
            shard.bytes()
        ),
        Err(ContractError::InvalidSourceIdentity(_))
    ));
}

#[test]
fn unknown_framework_facts_cannot_claim_participation() {
    let mut unknown = example_facts();
    unknown.classes[0].metaclass = MetaclassFact::Dynamic;
    assert!(matches!(
        encode_module_shard(&unknown),
        Err(ContractError::InvalidStructure(_))
    ));
    unknown.classes[0].participation = ParticipationProposal::Dynamic(BTreeSet::from([
        DynamicClassReason::NonParticipatingMetaclass,
    ]));
    encode_module_shard(&unknown).expect("dynamic metaclass facts are retained");
}

fn ignored_region(facts: &ModuleTypeFacts, scope: DiagnosticScope) -> StrictDiagnostic {
    let source_range = match &scope {
        DiagnosticScope::Module => SourceRange::new(0, facts.source_size),
        DiagnosticScope::Definition(identity) => identity.source_range,
        DiagnosticScope::Site(range) => *range,
    };
    StrictDiagnostic {
        code: DiagnosticCode::CheckerError,
        severity: DiagnosticSeverity::Error,
        source_range,
        scope,
        related_definitions: Vec::new(),
        suppressed: true,
        message: "ignored type error must not become a runtime proof".into(),
    }
}

#[test]
fn module_suppression_retains_only_dynamic_proposals() {
    let mut facts = example_facts();
    facts
        .diagnostics
        .push(ignored_region(&facts, DiagnosticScope::Module));
    assert!(matches!(
        validate_module_facts(&facts, Some(SOURCE)),
        Err(ContractError::BlockingDiagnostic(_))
    ));
    let fixture = Fixture::new(facts);
    let verified = fixture.verify_module();
    let dynamic = verified.facts();
    assert!(matches!(
        dynamic.classes[0].participation,
        ParticipationProposal::Dynamic(ref reasons)
            if reasons.contains(&DynamicClassReason::IgnoredDiagnostic)
    ));
    assert_eq!(
        dynamic.classes[0].instance_fields[0].value_type,
        StaticType::Unknown
    );
    assert_eq!(
        dynamic.functions[0].signature.return_type,
        StaticType::Unknown
    );
    assert_eq!(dynamic.call_sites[0].uncertainty, CallUncertainty::Dynamic);
    assert_eq!(
        dynamic.call_sites[0].candidate_targets,
        [CallableTargetFact::Dynamic]
    );
    assert_eq!(dynamic.global_bindings[0].value_type, StaticType::Unknown);
    assert_eq!(
        dynamic.global_bindings[0].mutability,
        GlobalMutability::FinalAfterSeal
    );
    assert!(dynamic.diagnostics[0].suppressed);
    assert_eq!(
        dynamic,
        &dynamic.canonicalized().expect("idempotent demotion")
    );
}

#[test]
fn ignored_method_region_demotes_its_class_and_callers_not_unrelated_classes() {
    let mut facts = facts_with_nominal_binding();
    // A separate lexical class with no dependencies on Box remains eligible.
    let mut other = facts.classes[0].clone();
    other.identity.lexical_qualname = "Independent".into();
    other.identity.source_range = range(SOURCE, "class Independent:\n    pass");
    other.instance_fields.clear();
    other.methods.clear();
    facts.classes.push(other);
    facts.diagnostics.push(ignored_region(
        &facts,
        DiagnosticScope::Site(range(SOURCE, "self.value")),
    ));
    let fixture = Fixture::new(facts);
    let verified = fixture.verify_module();
    let dynamic = verified.facts();
    assert!(dynamic.nominal_bindings.is_empty());
    let independent = dynamic
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "Independent")
        .expect("independent class");
    assert_eq!(independent.participation, ParticipationProposal::Candidate);
    let affected = dynamic
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "Box")
        .expect("affected class");
    assert!(matches!(
        affected.participation,
        ParticipationProposal::Dynamic(_)
    ));
    assert!(
        affected
            .uncertainty
            .contains(&UncertaintyReason::IgnoredDiagnostic)
    );
    assert_eq!(dynamic.call_sites[0].uncertainty, CallUncertainty::Dynamic);
    assert_eq!(
        dynamic.attribute_sites[0].receiver_type,
        StaticType::Unknown
    );
    assert_eq!(
        dynamic
            .functions
            .iter()
            .find(|function| function.identity.lexical_qualname == "read")
            .expect("caller")
            .signature
            .parameters[0]
            .value_type,
        StaticType::Unknown
    );
}

#[test]
fn unsuppressed_errors_and_mislocated_diagnostic_scopes_are_rejected() {
    let mut facts = example_facts();
    let mut diagnostic = ignored_region(
        &facts,
        DiagnosticScope::Definition(facts.classes[0].identity.clone()),
    );
    diagnostic.suppressed = false;
    facts.diagnostics.push(diagnostic);
    assert!(matches!(
        encode_module_shard(&facts),
        Err(ContractError::BlockingDiagnostic(_))
    ));
    facts.diagnostics[0].suppressed = true;
    facts.diagnostics[0].source_range = range(SOURCE, "box.get()");
    assert!(matches!(
        encode_module_shard(&facts),
        Err(ContractError::InvalidSourceIdentity(_))
    ));
}

#[test]
fn a_trusted_signature_cannot_preserve_precise_suppressed_facts() {
    let mut facts = example_facts().canonicalized().expect("canonical facts");
    facts
        .diagnostics
        .push(ignored_region(&facts, DiagnosticScope::Module));
    let raw = canonical_bytes(&facts).expect("unvalidated producer bytes");
    let mut index = ModuleArtifactIndex::from_shard(
        &encode_module_shard(&example_facts()).expect("valid shard"),
    )
    .expect("index");
    index.shard_digest = Fingerprint::digest(&raw);
    let manifest = TypeArtifactManifest::new(environment(), vec![index])
        .expect("signed index alone cannot inspect its shard");
    let verified = verify_manifest(
        &sign_manifest(&manifest, &signing_key()).expect("signed index"),
        &signing_key().trust_anchor(),
        &ArtifactExpectations {
            generation: manifest.generation,
            environment: environment(),
        },
    )
    .expect("authenticated manifest");
    assert!(matches!(
        verified.verify_module("pkg.example", SOURCE, &facts.language_policy, &[], &raw),
        Err(ContractError::BlockingDiagnostic(_))
    ));
}

#[test]
fn ordinary_source_does_not_acquire_strict_contracts_from_shared_policy() {
    let mut facts = example_facts();
    facts.source_dialect = SourceDialect::OrdinaryPython;
    assert!(matches!(
        encode_module_shard(&facts),
        Err(ContractError::InvalidStructure(_))
    ));
    facts.global_bindings[0].mutability = GlobalMutability::Unknown;
    facts.classes[0].participation =
        ParticipationProposal::Dynamic(BTreeSet::from([DynamicClassReason::UnresolvedAnalysis]));
    encode_module_shard(&facts).expect("ordinary semantic facts can remain dynamic");
}

#[test]
fn signatures_preserve_python_binding_order_and_async_kind_without_check_claims() {
    let mut facts = example_facts();
    let mut positional = parameter("first", nominal_int());
    positional.kind = ParameterKind::PositionalOnly;
    let positional_or_keyword = parameter("second", nominal_int());
    let mut varargs = parameter("args", StaticType::Unknown);
    varargs.kind = ParameterKind::VarArgs;
    let mut keyword = parameter("named", StaticType::Unknown);
    keyword.kind = ParameterKind::KeywordOnly;
    let mut varkw = parameter("kwargs", StaticType::Unknown);
    varkw.kind = ParameterKind::VarKeywords;
    facts.functions[1].signature.parameters =
        vec![positional, positional_or_keyword, varargs, keyword, varkw];
    facts.functions[1].function_kind = FunctionKind::Coroutine;
    let shard = encode_module_shard(&facts).expect("valid binding shape");
    let function = shard
        .facts()
        .functions
        .iter()
        .find(|function| function.identity.lexical_qualname == "read")
        .expect("read function");
    assert_eq!(function.function_kind, FunctionKind::Coroutine);
    assert_eq!(
        function.signature.parameters[0].kind,
        ParameterKind::PositionalOnly
    );
    assert_eq!(
        function.signature.parameters[3].kind,
        ParameterKind::KeywordOnly
    );
    facts.functions[1].signature.parameters.swap(0, 3);
    assert!(matches!(
        encode_module_shard(&facts),
        Err(ContractError::InvalidStructure(_))
    ));
}

#[test]
fn numeric_widening_and_mutable_generics_never_imply_exact_native_operands() {
    let widening = StaticType::NumericWidening {
        target: BuiltinType::Float,
        accepted: BTreeSet::from([BuiltinType::Int, BuiltinType::Float]),
    };
    assert!(widening.has_supported_value_shape());
    assert_ne!(widening, StaticType::ExactBuiltin(BuiltinType::Float));
    let generic = StaticType::Unsupported {
        kind: UnsupportedTypeKind::MutableGeneric,
        reason: UnsupportedReasonCode::AliasedMutableContents,
    };
    assert!(generic.contains_uncertainty());
    assert!(!generic.has_supported_value_shape());
    let mut facts = example_facts();
    facts.functions[1].signature.return_type = StaticType::NumericWidening {
        target: BuiltinType::Float,
        accepted: BTreeSet::from([BuiltinType::Float]),
    };
    assert!(matches!(
        encode_module_shard(&facts),
        Err(ContractError::InvalidType(_))
    ));
}

#[test]
fn dataclass_schema_preserves_pseudofields_defaults_and_generated_origins() {
    let mut facts = example_facts();
    let dependency = dependency("dataclasses", b"def dataclass(cls): ...\n");
    let decorator_definition = SourceIdentity {
        module: dependency.module.clone(),
        lexical_qualname: "dataclass".into(),
        source_range: SourceRange::new(0, dependency.source_size),
        definition_kind: DefinitionKind::Function,
    };
    facts.consumed_dependencies.push(dependency.clone());
    let class = &mut facts.classes[0];
    class.decorators.push(DecoratorFact {
        kind: DecoratorKind::StdlibDataclass,
        expression_range: class.identity.source_range,
        definition: Some(decorator_definition.clone()),
        source_digest: Some(dependency.source_digest),
        arguments: BTreeMap::new(),
        uncertainty: BTreeSet::new(),
    });
    class.transform = Some(ClassTransformFact {
        kind: TransformKind::StdlibDataclass,
        provenance: Some(decorator_definition),
        dataclass_options: Some(DataclassOptions::default()),
        generated_methods: BTreeSet::from(["__init__".into()]),
    });
    let declaring_class = class.instance_fields[0].declaring_class.clone();
    class.methods.push(MethodTypeFact {
        name: "__init__".into(),
        declaring_class: declaring_class.clone(),
        binding: MethodBinding::Instance,
        signature: signature(Vec::new()),
        declared_final: false,
        override_policy: OverridePolicy::CompatibleSignatureRequired,
        implementation: None,
        generated: Some(GeneratedFunctionFact {
            class: declaring_class,
            transform: TransformKind::StdlibDataclass,
            name: "__init__".into(),
        }),
        uncertainty: BTreeSet::new(),
    });
    let mut init_only = class.instance_fields[0].clone();
    init_only.name = "seed".into();
    init_only.field_kind = FieldKind::InitOnly;
    init_only.write_policy = FieldWritePolicy::InitOnly;
    let mut classvar = class.instance_fields[0].clone();
    classvar.name = "marker".into();
    classvar.field_kind = FieldKind::ClassVariable;
    classvar.write_policy = FieldWritePolicy::ClassVariableRejected;
    class.instance_fields[0].default = DefaultFact::Factory {
        implementation: None,
        return_type: Box::new(nominal_int()),
    };
    class.instance_fields.extend([init_only, classvar]);
    let shard = encode_module_shard(&facts).expect("dataclass facts");
    let class = &shard.facts().classes[0];
    assert_eq!(
        class.dictionary,
        ClassDictionarySemantics::DictionaryBearing
    );
    assert!(
        !class
            .transform
            .as_ref()
            .expect("transform")
            .dataclass_options
            .as_ref()
            .expect("options")
            .slots
    );
    assert_eq!(class.instance_fields[1].field_kind, FieldKind::InitOnly);
    assert_eq!(
        class.instance_fields[2].field_kind,
        FieldKind::ClassVariable
    );
    assert!(matches!(
        class.instance_fields[0].default,
        DefaultFact::Factory { .. }
    ));
    let generated = class
        .methods
        .iter()
        .find(|method| method.name == "__init__")
        .expect("generated init");
    assert!(generated.implementation.is_none());
    assert!(generated.generated.is_some());

    facts.language_policy.adapters.dataclasses = StdlibDataclassPolicy::Dynamic;
    assert!(matches!(
        encode_module_shard(&facts),
        Err(ContractError::InvalidPolicy(_))
    ));
    facts.classes[0].participation =
        ParticipationProposal::Dynamic(BTreeSet::from([DynamicClassReason::FrameworkManaged]));
    encode_module_shard(&facts).expect("disabled adapter still permits dynamic dataclass facts");
}

#[test]
fn corruption_noncanonical_encoding_and_unknown_fields_are_rejected() {
    let fixture = Fixture::new(example_facts());
    let mut corrupt = fixture.shard.bytes().to_vec();
    corrupt[0] ^= 1;
    assert!(matches!(
        fixture.verify().verify_module(
            "pkg.example",
            SOURCE,
            &fixture.facts.language_policy,
            &[],
            &corrupt
        ),
        Err(ContractError::ShardMismatch(_))
    ));
    let mut noncanonical = fixture.signed.clone();
    noncanonical.push(b'\n');
    assert!(matches!(
        verify_manifest(
            &noncanonical,
            &signing_key().trust_anchor(),
            &fixture.expected
        ),
        Err(ContractError::NonCanonicalEncoding)
    ));
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&fixture.signed).expect("envelope");
    envelope["manifest"]["versions"]["attacker_selected_mode"] = true.into();
    assert!(matches!(
        verify_manifest(
            &canonical_bytes(&envelope).expect("envelope"),
            &signing_key().trust_anchor(),
            &fixture.expected
        ),
        Err(ContractError::Encoding(_))
    ));
}

#[test]
fn imported_constant_definitions_are_authenticated_dependency_references() {
    let mut facts = example_facts();
    let external = dependency("pkg.constants", b"COUNT = 3\n");
    facts.global_bindings.push(GlobalBindingFact {
        name: "COUNT".into(),
        mutability: GlobalMutability::FinalAfterSeal,
        value_type: nominal_int(),
        definition: Some(SourceIdentity {
            module: external.module.clone(),
            lexical_qualname: "COUNT".into(),
            source_range: SourceRange::new(0, external.source_size),
            definition_kind: DefinitionKind::Assignment,
        }),
        uncertainty: BTreeSet::new(),
    });
    facts.consumed_dependencies.push(external);
    Fixture::new(facts).verify_module();
}

fn facts_with_builtin_base_catalog() -> ModuleTypeFacts {
    let mut facts = example_facts();
    let mut independent = facts.classes[0].clone();
    independent.identity = definition(
        &facts,
        "Independent",
        SourceRange::new(
            range(SOURCE, "class Independent:").start,
            range(SOURCE, "class Box:").start,
        ),
        DefinitionKind::Class,
    );
    independent.instance_fields.clear();
    independent.methods.clear();
    independent.class_members.clear();
    independent.inheritance.linearized_bases = vec![BaseReference::Builtin(BuiltinType::Object)];
    facts.classes.push(independent);
    facts
}

#[test]
fn builtin_base_references_preserve_order_and_distinct_authenticated_identity() {
    let mut facts = facts_with_builtin_base_catalog();
    let source_base = BaseReference::Class(ClassReference {
        definition: facts.classes[1].identity.clone(),
        source_digest: facts.source_digest,
    });
    let object = BaseReference::Builtin(BuiltinType::Object);
    facts.classes[0].bases = vec![source_base.clone(), object.clone()];
    facts.classes[0].inheritance.linearized_bases = facts.classes[0].bases.clone();
    facts.classes[1].inheritance.linearized_bases = vec![object.clone()];
    let fixture = Fixture::new(facts.clone());
    let verified = fixture.verify_module();
    assert_eq!(
        verified.facts().classes[0].bases,
        vec![source_base.clone(), object.clone()]
    );
    assert!(source_base.as_class().is_some());
    assert!(object.as_class().is_none());

    facts.classes[0].bases = vec![object.clone()];
    facts.classes[0].inheritance.linearized_bases = vec![object];
    let builtin_only = Fixture::new(facts);
    builtin_only.verify_module();
    assert_ne!(fixture.shard.digest(), builtin_only.shard.digest());
    assert_ne!(
        fixture.manifest.generation,
        builtin_only.manifest.generation
    );
}

#[test]
fn builtin_base_references_reject_inconsistent_mro_and_source_member_claims() {
    for mutation in 0..7 {
        let mut facts = facts_with_builtin_base_catalog();
        let source_base = ClassReference {
            definition: facts.classes[1].identity.clone(),
            source_digest: facts.source_digest,
        };
        let object = BaseReference::Builtin(BuiltinType::Object);
        facts.classes[0].bases = vec![object.clone()];
        facts.classes[0].inheritance.linearized_bases = vec![object.clone()];
        match mutation {
            0 => facts.classes[0].bases.push(object),
            1 => facts.classes[0].inheritance.linearized_bases.push(object),
            2 => facts.classes[0]
                .bases
                .push(BaseReference::Class(source_base)),
            3 => facts.classes[0]
                .inheritance
                .linearized_bases
                .push(BaseReference::Class(source_base)),
            4 => {
                let own = BaseReference::Class(ClassReference {
                    definition: facts.classes[0].identity.clone(),
                    source_digest: facts.source_digest,
                });
                facts.classes[0].bases = vec![own.clone()];
                facts.classes[0].inheritance.linearized_bases = vec![own, object];
            }
            5 => {
                let mut wrong = source_base;
                wrong.source_digest = fingerprint("different source base");
                facts.classes[0].bases = vec![BaseReference::Class(wrong.clone())];
                facts.classes[0].inheritance.linearized_bases =
                    vec![BaseReference::Class(wrong), object];
            }
            6 => facts.classes[0].instance_fields[0].declaring_class = source_base,
            _ => unreachable!(),
        }
        let error = validate_module_facts(&facts, Some(SOURCE)).unwrap_err();
        if mutation == 5 {
            assert!(matches!(error, ContractError::SourceMismatch(_)));
        } else {
            assert!(
                matches!(error, ContractError::InvalidStructure(_)),
                "mutation {mutation}"
            );
        }
    }
}

#[test]
fn builtin_base_references_require_explicit_schema_tags_without_name_conversion() {
    let class = ClassReference {
        definition: facts_with_builtin_base_catalog().classes[1]
            .identity
            .clone(),
        source_digest: Fingerprint::digest(SOURCE),
    };
    for reference in [
        BaseReference::Class(class.clone()),
        BaseReference::Builtin(BuiltinType::Object),
    ] {
        let encoded = serde_json::to_value(&reference).unwrap();
        assert_eq!(
            serde_json::from_value::<BaseReference>(encoded).unwrap(),
            reference
        );
    }
    for untagged in [
        serde_json::to_value(class).unwrap(),
        serde_json::json!({"kind": "builtin", "data": "builtins.object"}),
        serde_json::json!({"kind": "builtin", "data": "object", "source_digest": "guessed"}),
    ] {
        assert!(serde_json::from_value::<BaseReference>(untagged).is_err());
    }
}

#[test]
fn logical_inheritance_cycles_are_rejected_before_publication() {
    let source = b"from __future__ import strict\nclass A:\n    pass\nclass B:\n    pass\n";
    let mut facts = ModuleTypeFacts::new(
        "pkg.cycle",
        source,
        SourceDialect::SoacStrict,
        ResolvedStrictPolicy::default(),
    )
    .expect("module");
    let first_start = range(source, "class A:").start;
    let second_start = range(source, "class B:").start;
    let first = ClassReference {
        definition: definition(
            &facts,
            "A",
            SourceRange::new(first_start, second_start),
            DefinitionKind::Class,
        ),
        source_digest: facts.source_digest,
    };
    let second = ClassReference {
        definition: definition(
            &facts,
            "B",
            SourceRange::new(second_start, facts.source_size),
            DefinitionKind::Class,
        ),
        source_digest: facts.source_digest,
    };
    let class = |identity: SourceIdentity, base: ClassReference| ClassTypeFact {
        identity,
        bases: vec![BaseReference::Class(base.clone())],
        metaclass: MetaclassFact::BuiltinType,
        decorators: Vec::new(),
        participation: ParticipationProposal::Candidate,
        dictionary: ClassDictionarySemantics::DictionaryBearing,
        instance_fields: Vec::new(),
        methods: Vec::new(),
        class_members: Vec::new(),
        inheritance: InheritanceFact {
            linearized_bases: vec![BaseReference::Class(base)],
            complete: true,
        },
        openness: ClassOpenness::OpenSubclassFamily,
        transform: None,
        uncertainty: BTreeSet::new(),
    };
    facts.classes = vec![
        class(first.definition.clone(), second.clone()),
        class(second.definition, first),
    ];
    assert!(matches!(
        encode_module_shard(&facts),
        Err(ContractError::InvalidStructure(_))
    ));
}

#[test]
fn callable_fields_and_method_implementations_cannot_change_binding_categories() {
    let mut method = example_facts();
    let unrelated = method.functions[1].identity.clone();
    let CallableTargetFact::Method { implementation, .. } =
        &mut method.call_sites[0].candidate_targets[0]
    else {
        panic!("method target")
    };
    *implementation = Some(unrelated);
    assert!(matches!(
        encode_module_shard(&method),
        Err(ContractError::InvalidStructure(_))
    ));

    let mut field = example_facts();
    field.call_sites[0].uncertainty = CallUncertainty::CallableInstanceField;
    assert!(matches!(
        encode_module_shard(&field),
        Err(ContractError::InvalidStructure(_))
    ));

    for binding in [
        CallBindingFact::BoundClassMethod,
        CallBindingFact::StaticMethod,
        CallBindingFact::CallableInstanceField,
    ] {
        let mut facts = example_facts();
        facts.call_sites[0].binding = binding;
        assert!(
            matches!(
                encode_module_shard(&facts),
                Err(ContractError::InvalidStructure(_))
            ),
            "mismatched {binding:?} binding must not become a method call plan"
        );
    }
}

#[test]
fn one_generation_cannot_assign_conflicting_external_dependency_identities() {
    let mut first = example_facts();
    let external = dependency("pkg.external", b"class External: ...\n");
    first.consumed_dependencies.push(external.clone());
    let first = encode_module_shard(&first).expect("first shard");
    let mut second = ModuleTypeFacts::new(
        "pkg.other",
        b"from __future__ import strict\n",
        SourceDialect::SoacStrict,
        ResolvedStrictPolicy::default(),
    )
    .expect("second module");
    let mutations: &[fn(&mut DependencyFingerprint)] = &[
        |value| value.module.source_hash ^= 1,
        |value| value.source_digest = fingerprint("different external source"),
        |value| value.source_size += 1,
        |value| value.import_resolution = fingerprint("different external resolution"),
        |value| value.effective_configuration = fingerprint("different external configuration"),
    ];
    for mutation in mutations {
        let mut changed = external.clone();
        mutation(&mut changed);
        second.consumed_dependencies = vec![changed];
        let second = encode_module_shard(&second).expect("individually valid consumer");
        assert!(matches!(
            TypeArtifactManifest::new(
                environment(),
                vec![
                    ModuleArtifactIndex::from_shard(&first).expect("first index"),
                    ModuleArtifactIndex::from_shard(&second).expect("second index")
                ]
            ),
            Err(ContractError::DependencyMismatch(_))
        ));
    }
}

#[test]
fn optional_dependency_contracts_cannot_hide_conflicts_between_consumers() {
    let producer = dependency("pkg.external", b"class External: ...\n");
    let mut indices = Vec::new();
    for (name, type_contract) in [
        ("pkg.a_first", Some(fingerprint("contract A"))),
        ("pkg.b_middle", None),
        ("pkg.c_last", Some(fingerprint("contract B"))),
    ] {
        let mut facts = ModuleTypeFacts::new(
            name,
            b"from __future__ import strict\n",
            SourceDialect::SoacStrict,
            ResolvedStrictPolicy::default(),
        )
        .expect("module");
        let mut dependency = producer.clone();
        dependency.type_contract = type_contract;
        facts.consumed_dependencies.push(dependency);
        indices.push(
            ModuleArtifactIndex::from_shard(&encode_module_shard(&facts).expect("shard"))
                .expect("index"),
        );
    }
    assert!(matches!(
        TypeArtifactManifest::new(environment(), indices),
        Err(ContractError::DependencyMismatch(_))
    ));
}

struct AnalysisInputFixture(std::path::PathBuf);

impl AnalysisInputFixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        loop {
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "soac-contract-inputs-{}-{sequence}",
                std::process::id()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create analysis input fixture: {error}"),
            }
        }
    }

    fn capture(&self, relative: &str, enumerate: bool) -> AnalysisInput {
        let path = self.0.join(relative);
        AnalysisInput {
            state: capture_analysis_input(&path, enumerate).expect("capture fixture input"),
            path,
        }
    }
}

impl Drop for AnalysisInputFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn analysis_input_verification_detects_same_size_edits_and_disappearance() {
    let fixture = AnalysisInputFixture::new();
    let path = fixture.0.join("module.py");
    std::fs::write(&path, b"value = 1\n").expect("write source");
    let input = fixture.capture("module.py", false);
    verify_analysis_inputs(std::slice::from_ref(&input)).expect("unchanged source");
    std::fs::write(&path, b"value = 2\n").expect("same-size source change");
    assert!(verify_analysis_inputs(std::slice::from_ref(&input)).is_err());
    std::fs::remove_file(&path).expect("remove source");
    assert!(verify_analysis_inputs(&[input]).is_err());
}

#[test]
fn analysis_input_verification_retains_negative_import_resolution_observations() {
    let fixture = AnalysisInputFixture::new();
    let missing = fixture.capture("missing.pyi", false);
    assert_eq!(missing.state, AnalysisInputState::Missing);
    verify_analysis_inputs(std::slice::from_ref(&missing)).expect("still absent");
    std::fs::write(&missing.path, b"value: int\n").expect("new resolver candidate");
    assert!(verify_analysis_inputs(&[missing]).is_err());
}

#[test]
fn analysis_directory_verification_checks_only_observed_enumerations() {
    let fixture = AnalysisInputFixture::new();
    let enumerated = fixture.capture("", true);
    let existence_only = fixture.capture("", false);
    verify_analysis_inputs(std::slice::from_ref(&enumerated)).expect("unchanged directory");
    std::fs::write(fixture.0.join("added.py"), b"pass\n").expect("new directory entry");
    assert!(verify_analysis_inputs(&[enumerated]).is_err());
    verify_analysis_inputs(&[existence_only])
        .expect("unobserved children are not claimed as facts");

    let with_file = fixture.capture("", true);
    std::fs::remove_file(fixture.0.join("added.py")).expect("remove file entry");
    std::fs::create_dir(fixture.0.join("added.py")).expect("same name now denotes a directory");
    assert!(verify_analysis_inputs(&[with_file]).is_err());
}

#[cfg(unix)]
#[test]
fn analysis_symlink_verification_binds_same_byte_files_and_directories_to_their_targets() {
    use std::os::unix::fs::symlink;

    let fixture = AnalysisInputFixture::new();
    for target in ["first.py", "second.py"] {
        std::fs::write(fixture.0.join(target), b"value = 1\n").expect("identical source files");
    }
    let alias = fixture.0.join("current.py");
    symlink("first.py", &alias).expect("initial source symlink");
    let original = fixture.capture("current.py", false);
    std::fs::remove_file(&alias).expect("remove source symlink");
    symlink("second.py", &alias).expect("retarget identical source");
    assert!(verify_analysis_inputs(&[original]).is_err());

    for target in ["first", "second"] {
        std::fs::create_dir(fixture.0.join(target)).expect("empty resolver roots");
    }
    let alias = fixture.0.join("root");
    symlink("first", &alias).expect("initial directory symlink");
    let original = fixture.capture("root", true);
    std::fs::remove_file(&alias).expect("remove directory symlink");
    symlink("second", &alias).expect("retarget identical directory");
    assert!(verify_analysis_inputs(&[original]).is_err());
}

#[test]
fn filtered_directory_queries_ignore_unconsumed_names_but_reject_new_import_candidates() {
    for (filter, added) in [
        (
            AnalysisDirectoryFilter::Name {
                name: "value.py".into(),
            },
            "value.py",
        ),
        (
            AnalysisDirectoryFilter::Prefix {
                prefix: "value".into(),
            },
            "value.pyi",
        ),
        (
            AnalysisDirectoryFilter::Suffix {
                suffix: ".pth".into(),
            },
            "editable.pth",
        ),
        (
            AnalysisDirectoryFilter::Suffix {
                suffix: ".dist-info".into(),
            },
            "installed.dist-info",
        ),
    ] {
        let fixture = AnalysisInputFixture::new();
        let input = AnalysisInput {
            path: fixture.0.clone(),
            state: capture_analysis_input_with_filters(&fixture.0, &[filter]).unwrap(),
        };
        std::fs::write(fixture.0.join("deployment.json"), b"{}").unwrap();
        std::fs::create_dir(fixture.0.join("__pycache__")).unwrap();
        verify_analysis_inputs(std::slice::from_ref(&input))
            .expect("unconsumed names do not change the resolver query");
        std::fs::write(fixture.0.join(added), b"new candidate").unwrap();
        assert!(verify_analysis_inputs(&[input]).is_err());
    }
}

#[test]
fn discovery_exclusions_do_not_hide_direct_imports_into_cache_directories() {
    let fixture = AnalysisInputFixture::new();
    let discovery = AnalysisInput {
        path: fixture.0.clone(),
        state: capture_analysis_input_with_filters(
            &fixture.0,
            &[AnalysisDirectoryFilter::SourceSelection {
                excluded_names: vec!["__pycache__".into()],
            }],
        )
        .unwrap(),
    };
    let imported = fixture.capture("__pycache__/module.py", false);
    std::fs::create_dir(fixture.0.join("__pycache__")).unwrap();
    std::fs::write(&imported.path, b"value = 1\n").unwrap();
    verify_analysis_inputs(&[discovery]).expect("cache is excluded only from source discovery");
    assert!(verify_analysis_inputs(&[imported]).is_err());
}

fn deployment_fixture() -> (AnalysisInputFixture, StrictArtifactDeployment) {
    let fixture = AnalysisInputFixture::new();
    for (name, bytes) in [
        ("python", b"test executable identity".as_slice()),
        ("pyconfig.h", b"test target configuration".as_slice()),
        ("model.py", SOURCE),
        ("other.py", SOURCE),
        ("external.py", b"VALUE = 1\n".as_slice()),
        ("same.py", b"VALUE = 1\n".as_slice()),
    ] {
        std::fs::write(fixture.0.join(name), bytes).unwrap();
    }
    let inputs = [
        "python",
        "pyconfig.h",
        "model.py",
        "other.py",
        "external.py",
        "same.py",
    ]
    .map(|name| fixture.capture(name, false))
    .to_vec();
    let target = InterpreterIdentity {
        version: [3, 15],
        platform: "linux-aarch64".into(),
        prefix: fixture.0.to_str().unwrap().into(),
        executable: fixture.0.join("python").to_str().unwrap().into(),
        build_directory: fixture.0.to_str().unwrap().into(),
        site_packages: Vec::new(),
        real_stdlib: fixture.0.to_str().unwrap().into(),
        abi_files: vec![fixture.0.join("python").to_str().unwrap().into()],
        configuration_files: vec![fixture.0.join("pyconfig.h").to_str().unwrap().into()],
        configuration: BTreeMap::new(),
    };
    let mut environment = environment();
    environment.cpython_abi_fingerprint = target.abi_fingerprint(&inputs).unwrap();
    let dependency = AnalysisDependency {
        importer_module: "model".into(),
        module: ModuleContentId::new("external", legacy_source_hash(b"VALUE = 1\n")),
        source: AnalysisDependencySource::System {
            path: fixture.0.join("external.py"),
        },
        source_digest: Fingerprint::digest(b"VALUE = 1\n"),
        source_size: b"VALUE = 1\n".len() as u32,
        configuration: AnalysisFileConfiguration {
            python_version: environment.python_version,
            python_platform: environment.python_platform.clone(),
            analysis: ConservativeAnalysis::default(),
            respect_type_ignore_comments: true,
            import_search_paths: vec![fixture.0.to_str().unwrap().into()],
            enabled_diagnostics: BTreeMap::from([("invalid-assignment".into(), "error".into())]),
        },
    };
    let deployment = StrictArtifactDeployment {
        schema_version: DEPLOYMENT_SCHEMA_VERSION,
        artifact_directory: fixture.0.join("artifacts/generation"),
        generation: ArtifactGenerationId::new(fingerprint("test deployment")),
        environment,
        target_interpreter: target,
        trust_anchor: signing_key().trust_anchor().to_bytes(),
        modules: ["model", "other"]
            .map(|name| DeployedModule {
                module_name: name.into(),
                source_path: fixture.0.join(format!("{name}.py")),
                policy: ResolvedStrictPolicy::default(),
            })
            .to_vec(),
        analysis_dependencies: vec![dependency],
        analysis_inputs: inputs,
        analysis_environment: Vec::new(),
    };
    deployment.validate().unwrap();
    (fixture, deployment)
}

#[test]
fn startup_interpreter_identity_requires_the_observed_abi_files_and_environment() {
    let (_fixture, deployment) = deployment_fixture();
    let identity = &deployment.target_interpreter;
    assert_eq!(
        identity
            .abi_fingerprint(&deployment.analysis_inputs)
            .unwrap(),
        deployment.environment.cpython_abi_fingerprint
    );
    let mut changed = deployment.clone();
    changed
        .target_interpreter
        .configuration
        .insert("SOABI".into(), "different-abi".into());
    assert!(changed.validate().is_err());
    let mut changed = deployment.clone();
    changed
        .analysis_inputs
        .retain(|input| input.path != std::path::Path::new(&identity.executable));
    assert!(changed.validate().is_err());
    let mut changed = deployment.clone();
    changed
        .target_interpreter
        .abi_files
        .push(identity.executable.clone());
    assert!(changed.validate().is_err());
    let mut changed = deployment.clone();
    changed.target_interpreter.version = [3, 14];
    assert!(changed.validate().is_err());
}

#[test]
fn dependency_expectations_rebuild_source_identity_and_reject_stale_bytes() {
    let (fixture, deployment) = deployment_fixture();
    let dependencies = deployment.verified_analysis_dependencies("model").unwrap();
    assert_eq!(dependencies.len(), 1);
    assert_eq!(
        dependencies[0].module.source_hash,
        legacy_source_hash(b"VALUE = 1\n")
    );
    assert_eq!(dependencies[0].strict_policy, None);
    let mut changed = deployment.clone();
    changed.analysis_dependencies[0].module.source_hash ^= 1;
    assert!(changed.verified_analysis_dependencies("model").is_err());
    assert!(changed.verified_analysis_snapshot().is_err());
    assert!(
        deployment
            .verified_analysis_dependencies("not_selected")
            .is_err()
    );
    std::fs::write(fixture.0.join("external.py"), b"VALUE = 2\n").unwrap();
    assert!(deployment.verified_analysis_dependencies("model").is_err());
    assert!(deployment.verified_analysis_snapshot().is_err());
}

#[test]
fn analysis_snapshot_shares_actual_bytes_without_sharing_consumer_domains() {
    let (fixture, mut deployment) = deployment_fixture();
    let mut other = deployment.analysis_dependencies[0].clone();
    other.importer_module = "other".into();
    other.module.module_name = "same_file_different_module".into();
    other.configuration.respect_type_ignore_comments = false;
    deployment.analysis_dependencies.push(other);
    // The same source is strict when named by one consumer and ordinary when
    // resolved under the other name. Sharing bytes cannot share that policy.
    deployment.modules.push(DeployedModule {
        module_name: "external".into(),
        source_path: fixture.0.join("external.py"),
        policy: ResolvedStrictPolicy::default(),
    });
    let snapshot = deployment.verified_analysis_snapshot().unwrap();
    for module in &deployment.modules {
        assert_eq!(
            snapshot.dependencies(&module.module_name).unwrap(),
            deployment
                .verified_analysis_dependencies(&module.module_name)
                .unwrap()
        );
    }
    let model = &snapshot.dependencies("model").unwrap()[0];
    let other = &snapshot.dependencies("other").unwrap()[0];
    assert_eq!(model.source_digest, other.source_digest);
    assert_eq!(model.import_resolution, other.import_resolution);
    assert_ne!(model.module, other.module);
    assert_ne!(model.effective_configuration, other.effective_configuration);
    assert!(model.strict_policy.is_some());
    assert!(other.strict_policy.is_none());
    assert!(snapshot.dependencies("external").unwrap().is_empty());
    assert!(snapshot.dependencies("unselected").is_err());

    let mut changed = deployment.clone();
    changed.analysis_dependencies[1].module.source_hash ^= 1;
    // The first consumer's valid observation does not authenticate another
    // record's claimed legacy hash, even though both name the identical path.
    assert!(changed.verified_analysis_snapshot().is_err());
    assert!(changed.verified_analysis_dependencies("other").is_err());
}

#[test]
fn analysis_snapshot_checks_all_inputs_even_without_consumed_dependencies() {
    let (fixture, mut deployment) = deployment_fixture();
    deployment.analysis_dependencies.clear();
    assert!(
        deployment
            .verified_analysis_snapshot()
            .unwrap()
            .dependencies("model")
            .unwrap()
            .is_empty()
    );
    std::fs::write(fixture.0.join("external.py"), b"VALUE = 2\n").unwrap();
    assert!(deployment.verified_analysis_snapshot().is_err());
}

#[test]
fn source_path_role_consumer_and_per_file_policy_are_independent_of_signed_dependencies() {
    let (fixture, deployment) = deployment_fixture();
    let mut facts = ModuleTypeFacts::new(
        "model",
        SOURCE,
        SourceDialect::SoacStrict,
        ResolvedStrictPolicy::default(),
    )
    .unwrap();
    facts.consumed_dependencies = deployment.verified_analysis_dependencies("model").unwrap();
    let shard = encode_module_shard(&facts).unwrap();
    let manifest = TypeArtifactManifest::new(
        deployment.environment.clone(),
        vec![ModuleArtifactIndex::from_shard(&shard).unwrap()],
    )
    .unwrap();
    let signed = sign_manifest(&manifest, &signing_key()).unwrap();
    let verified = verify_manifest(
        &signed,
        &signing_key().trust_anchor(),
        &ArtifactExpectations {
            generation: manifest.generation,
            environment: deployment.environment.clone(),
        },
    )
    .unwrap();
    verified
        .verify_module(
            "model",
            SOURCE,
            &facts.language_policy,
            &facts.consumed_dependencies,
            shard.bytes(),
        )
        .unwrap();
    for mutation in 0..5 {
        let mut changed = deployment.clone();
        match mutation {
            0 => {
                changed.analysis_dependencies[0].source = AnalysisDependencySource::System {
                    path: fixture.0.join("same.py"),
                }
            }
            1 => {
                changed.analysis_dependencies[0].source = AnalysisDependencySource::Vendored {
                    path: "stdlib/external.pyi".into(),
                }
            }
            2 => changed.analysis_dependencies[0].importer_module = "other".into(),
            3 => {
                changed.analysis_dependencies[0]
                    .configuration
                    .respect_type_ignore_comments = false
            }
            4 => changed.modules.push(DeployedModule {
                module_name: "external".into(),
                source_path: fixture.0.join("external.py"),
                policy: ResolvedStrictPolicy::default(),
            }),
            _ => unreachable!(),
        }
        let expected = changed.verified_analysis_dependencies("model").unwrap();
        let snapshot = changed.verified_analysis_snapshot().unwrap();
        assert_eq!(snapshot.dependencies("model").unwrap(), expected);
        assert!(matches!(
            verified.verify_module(
                "model",
                SOURCE,
                &facts.language_policy,
                snapshot.dependencies("model").unwrap(),
                shard.bytes()
            ),
            Err(ContractError::DependencyMismatch(_))
        ));
    }
    let mut changed = deployment.clone();
    changed.analysis_dependencies[0]
        .configuration
        .analysis
        .strict_equality_semantics = false;
    assert!(changed.verified_analysis_dependencies("model").is_err());
    assert!(changed.verified_analysis_snapshot().is_err());
    let mut changed = deployment.clone();
    changed.analysis_dependencies[0].importer_module = "unselected".into();
    assert!(changed.validate().is_err());
    assert!(changed.verified_analysis_snapshot().is_err());
}

#[test]
fn vendored_dependency_binding_includes_checker_typeshed_path_and_source_role() {
    let (_fixture, mut deployment) = deployment_fixture();
    deployment.analysis_dependencies[0].source = AnalysisDependencySource::Vendored {
        path: "stdlib/external.pyi".into(),
    };
    let baseline = deployment.verified_analysis_dependencies("model").unwrap();
    for mutation in 0..4 {
        let mut changed = deployment.clone();
        match mutation {
            0 => changed.environment.checker_source_fingerprint = fingerprint("different checker"),
            1 => changed.environment.typeshed_fingerprint = fingerprint("different typeshed"),
            2 => {
                changed.analysis_dependencies[0].source = AnalysisDependencySource::Vendored {
                    path: "stdlib/different.pyi".into(),
                }
            }
            3 => {
                changed.analysis_dependencies[0].source_digest =
                    fingerprint("different bundled bytes")
            }
            _ => unreachable!(),
        }
        let snapshot = changed.verified_analysis_snapshot().unwrap();
        assert_eq!(
            snapshot.dependencies("model").unwrap(),
            changed.verified_analysis_dependencies("model").unwrap()
        );
        assert_ne!(
            snapshot.dependencies("model").unwrap()[0].import_resolution,
            baseline[0].import_resolution
        );
    }
    for path in [
        "/absolute.pyi",
        "../outside.pyi",
        "stdlib/../outside.pyi",
        "stdlib//empty.pyi",
    ] {
        let mut changed = deployment.clone();
        changed.analysis_dependencies[0].source =
            AnalysisDependencySource::Vendored { path: path.into() };
        assert!(changed.validate().is_err());
        assert!(changed.verified_analysis_snapshot().is_err());
    }
}

#[test]
fn revalidation_rejects_enlarged_files_before_allocating_their_contents() {
    let fixture = AnalysisInputFixture::new();
    let path = fixture.0.join("source.py");
    std::fs::write(&path, b"pass\n").unwrap();
    let input = fixture.capture("source.py", false);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(1 << 40)
        .unwrap();
    assert!(verify_analysis_inputs(&[input]).is_err());
}
