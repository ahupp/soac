//! End-to-end offline analysis through the vendored checker, selected
//! CPython process, signing/publication code, and shared artifact verifier.

use super::*;
use soac_contracts::{
    AnalysisInputState, AnnotationOrigin, ArtifactExpectations, ArtifactTrustAnchor,
    CheckedFieldPolicy, DefinitionKind, DynamicClassReason, GlobalMutability, ModuleTypeFacts,
    NominalBindingOwner, ParticipationProposal, StaticType, TransformKind,
    VerifiedTypeArtifactManifest, verify_complete_generation, verify_manifest,
};

struct Fixture {
    directory: tempfile::TempDir,
    project: PathBuf,
    python: PathBuf,
}

impl Fixture {
    fn new(source: &str) -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let project = directory.path().join("project");
        fs::create_dir(&project)?;
        fs::write(
            project.join("pyproject.toml"),
            "[project]\nname='offline-fixture'\nversion='0.0.0'\nrequires-python='>=3.15'\n",
        )?;
        fs::write(project.join("model.py"), source)?;
        keygen(&project.join("signing.key"))?;
        let python = std::env::var_os("CPYTHON_BIN").map(PathBuf::from)
            .context("real offline tests require the selected CPYTHON_BIN; run `just ty --debug-build --test`")?;
        Ok(Self {
            directory,
            project,
            python,
        })
    }

    fn options(&self) -> Check {
        Check {
            project: self.project.clone(),
            source_root: None,
            modules: Vec::new(),
            output: PathBuf::from("artifacts"),
            signing_key: PathBuf::from("signing.key"),
            deployment: PathBuf::from("deployment.json"),
            python: self.python.clone(),
            python_version: "3.15".into(),
        }
    }

    fn run(&self) -> Result<(publish::Publication, StrictArtifactDeployment)> {
        self.run_with_options(self.options())
    }

    fn run_with_options(
        &self,
        options: Check,
    ) -> Result<(publish::Publication, StrictArtifactDeployment)> {
        let publication = check(options)?;
        let deployment: StrictArtifactDeployment =
            serde_json::from_slice(&fs::read(self.project.join("deployment.json"))?)?;
        deployment.validate()?;
        verify_analysis_inputs(&deployment.analysis_inputs)?;
        Ok((publication, deployment))
    }

    fn manifest(
        &self,
        deployment: &StrictArtifactDeployment,
    ) -> Result<VerifiedTypeArtifactManifest> {
        Ok(verify_manifest(
            &fs::read(deployment.artifact_directory.join("manifest.json"))?,
            &ArtifactTrustAnchor::from_bytes(&deployment.trust_anchor)?,
            &ArtifactExpectations {
                generation: deployment.generation,
                environment: deployment.environment.clone(),
            },
        )?)
    }

    fn facts(&self, deployment: &StrictArtifactDeployment) -> Result<ModuleTypeFacts> {
        self.module_facts(deployment, "model")
    }

    fn module_facts(
        &self,
        deployment: &StrictArtifactDeployment,
        module_name: &str,
    ) -> Result<ModuleTypeFacts> {
        let manifest = self.manifest(deployment)?;
        let index = manifest.module_index(module_name)?;
        let module = deployment
            .modules
            .iter()
            .find(|module| module.module_name == module_name)
            .unwrap();
        let shard = fs::read(
            deployment
                .artifact_directory
                .join("modules")
                .join(format!("{}.soac-types", index.shard_digest)),
        )?;
        Ok(manifest
            .verify_module(
                module_name,
                &fs::read(&module.source_path)?,
                &module.policy,
                &deployment.verified_analysis_dependencies(module_name)?,
                &shard,
            )?
            .facts()
            .clone())
    }

    fn use_private_interpreter(&mut self) -> Result<Vec<PathBuf>> {
        let original = interpreter_identity(&self.python)?;
        let private = self.directory.path().join("interpreter");
        fs::create_dir(&private)?;
        let executable = private.join("python");
        fs::copy(&self.python, &executable)?;
        let mut libraries = Vec::new();
        for path in &original.abi_files {
            let path = Path::new(path);
            if path.canonicalize()? == self.python.canonicalize()? {
                continue;
            }
            let target = private.join(path.file_name().context("loaded library filename")?);
            fs::copy(path, &target)?;
            libraries.push(target);
        }
        ensure!(
            !libraries.is_empty(),
            "ABI test requires the selected shared CPython build"
        );
        let build = Path::new(&original.build_directory);
        let dynamic = build
            .join(fs::read_to_string(build.join("pybuilddir.txt"))?.trim())
            .canonicalize()?;
        fs::write(
            private.join("python._pth"),
            format!("{}\n{}\n", original.real_stdlib, dynamic.display()),
        )?;
        self.python = executable;
        let selected = interpreter_identity(&self.python)?;
        ensure!(
            selected
                .abi_files
                .iter()
                .any(|path| libraries.contains(&PathBuf::from(path))),
            "private probe did not load the copied library"
        );
        for site in selected.site_packages {
            fs::create_dir_all(site)?;
        }
        Ok(libraries)
    }
}

#[test]
fn offline_check_lambda_identities_keep_original_lexical_ranges() -> Result<()> {
    let fixture = Fixture::new("")?;
    let marker = fixture.project.join("lambda-source-was-imported.txt");
    let mut source = r#""""Original λ byte offsets"""
# soac: module(strict_assign=true, checked_attr=true)
from pathlib import Path
module_nested = lambda: (lambda: "nested")
module_generator = (lambda: index for index in range(2))
def classcell_values():
    class C:
        def method(self):
            super()
            return __class__
        items = [(lambda: item) for item in range(5)]
        y = [function() for function in items]
    return C.y, C().method(), C
"#
    .to_owned();
    source.push_str(&format!(
        "Path({}).write_text('imported')\n",
        serde_json::to_string(&marker)?
    ));
    fs::write(fixture.project.join("model.py"), &source)?;
    let (_, deployment) = fixture.run()?;
    assert!(
        !marker.exists(),
        "offline analysis must not execute the source"
    );
    let facts = fixture.facts(&deployment)?;
    for (expression, expected) in [
        ("lambda: (lambda: \"nested\")", "<lambda>"),
        ("lambda: \"nested\"", "<lambda>.<lambda>"),
        ("lambda: index", "<lambda>"),
        ("lambda: item", "classcell_values.<locals>.C.<lambda>"),
    ] {
        let start = source.find(expression).unwrap();
        let range =
            soac_contracts::SourceRange::new(start as u32, (start + expression.len()) as u32);
        let function = facts
            .functions
            .iter()
            .find(|function| function.identity.source_range == range)
            .unwrap();
        assert_eq!(function.identity.definition_kind, DefinitionKind::Lambda);
        assert_eq!(function.identity.lexical_qualname, expected);
        assert_eq!(function.identity.module, facts.module);
    }
    Ok(())
}

#[test]
fn offline_check_never_imports_source_and_repeated_publication_is_byte_identical() -> Result<()> {
    let fixture = Fixture::new("")?;
    let marker = fixture.project.join("source-was-imported.txt");
    let source = format!(
        "# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\nfrom pathlib import Path\n@dataclass\nclass Point:\n    x: int\n    y: int = 0\nclass Fields:\n    def __init__(self, value: int):\n        self.explicit: int = value\n        self.inferred = value\ndef identity(value: int) -> int: return value\nPath({}).write_text('imported')\nraise RuntimeError('this module must never execute during offline analysis')\n",
        serde_json::to_string(&marker)?
    );
    fs::write(fixture.project.join("model.py"), source)?;
    let (first, deployment) = fixture.run()?;
    assert!(!marker.exists());
    let first_manifest = fs::read(first.artifact_directory.join("manifest.json"))?;
    let facts = fixture.facts(&deployment)?;
    let point = facts
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "Point")
        .unwrap();
    assert_eq!(
        point.transform.as_ref().unwrap().kind,
        TransformKind::StdlibDataclass
    );
    for (name, parameter_names, return_type) in [
        ("__repr__", &["self"][..], soac_contracts::BuiltinType::Str),
        (
            "__eq__",
            &["self", "other"][..],
            soac_contracts::BuiltinType::Bool,
        ),
    ] {
        assert!(
            point
                .transform
                .as_ref()
                .unwrap()
                .generated_methods
                .contains(name)
        );
        let method = point
            .methods
            .iter()
            .find(|method| method.name == name)
            .unwrap();
        assert!(method.implementation.is_none());
        assert_eq!(
            method.generated.as_ref().unwrap().class.definition,
            point.identity
        );
        assert_eq!(
            method
                .signature
                .parameters
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect::<Vec<_>>(),
            parameter_names
        );
        assert!(method.signature.parameters.iter().all(|parameter| {
            parameter.kind == soac_contracts::ParameterKind::PositionalOrKeyword
                && parameter.annotation_origin == AnnotationOrigin::Inferred
        }));
        assert_eq!(
            method.signature.return_annotation_origin,
            AnnotationOrigin::Inferred
        );
        assert_eq!(
            method.signature.return_type,
            StaticType::NominalBuiltin {
                builtin: return_type,
                allow_subclasses: true,
            }
        );
    }
    assert_eq!(
        point
            .instance_fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        ["x", "y"]
    );
    assert!(
        point
            .instance_fields
            .iter()
            .all(|field| field.annotation_origin == AnnotationOrigin::Explicit)
    );
    let fields = facts
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "Fields")
        .unwrap();
    for (name, origin) in [
        ("explicit", AnnotationOrigin::Explicit),
        ("inferred", AnnotationOrigin::Inferred),
    ] {
        assert_eq!(
            fields
                .instance_fields
                .iter()
                .find(|field| field.name == name)
                .unwrap()
                .annotation_origin,
            origin
        );
    }
    assert!(
        facts
            .functions
            .iter()
            .any(|function| function.identity.lexical_qualname == "identity"
                && function.signature.parameters.len() == 1)
    );
    fs::create_dir(fixture.project.join("__pycache__"))?;
    fs::write(
        fixture.project.join("__pycache__/model.cpython-315.pyc"),
        b"irrelevant bytecode cache",
    )?;
    fs::write(fixture.project.join("unrelated-output.json"), b"{}")?;
    verify_analysis_inputs(&deployment.analysis_inputs)?;
    let (again, second) = fixture.run()?;
    assert_eq!(again.generation, first.generation);
    assert_eq!(again.reused_shards, 1);
    assert_eq!(
        fs::read(again.artifact_directory.join("manifest.json"))?,
        first_manifest
    );
    assert_eq!(fixture.facts(&second)?, facts);
    assert!(!marker.exists());
    Ok(())
}

#[test]
fn offline_check_imported_nominals_preserve_local_bindings_and_dependency_identity() -> Result<()> {
    let source = "# soac: module(strict_assign=true, checked_attr=true)\nfrom typing import Optional, Union\nfrom targets import Box\nfrom targets import Box as Alias\ndef field(owner: Box) -> int:\n    return owner.value\ndef call(owner: Box, extra: int) -> int:\n    return owner.read(extra)\ndef identity(owner: \"Box | Alias\", optional: Optional[Box] = None) -> Union[Box, Alias]:\n    return owner\n";
    let fixture = Fixture::new(source)?;
    let target_marker = fixture.directory.path().join("target-was-imported.txt");
    let target_source = format!(
        "# soac: module(strict_assign=true, checked_attr=true)\nfrom pathlib import Path\nclass Box:\n    value: int\n    def __init__(self, value: int):\n        self.value = value\n    def read(self, extra: int) -> int:\n        return self.value + extra\nPath({}).write_text('imported')\n",
        serde_json::to_string(&target_marker)?
    );
    fs::write(fixture.project.join("targets.py"), &target_source)?;
    let (first, deployment) = fixture.run()?;
    assert!(!target_marker.exists());
    let facts = fixture.facts(&deployment)?;
    let target = facts
        .consumed_dependencies
        .iter()
        .find(|dependency| dependency.module.module_name == "targets")
        .unwrap();
    assert_eq!(target.source_digest, Fingerprint::digest(&target_source));
    for (name, count) in [("field", 1), ("call", 1), ("identity", 5)] {
        let function = facts
            .functions
            .iter()
            .find(|function| function.identity.lexical_qualname == name)
            .unwrap();
        assert!(matches!(
            &function.signature.parameters[0].value_type,
            StaticType::NominalClass(class) if class.definition.module == target.module
        ));
        let leaves: Vec<_> = facts
            .nominal_bindings
            .iter()
            .filter(|leaf| {
                leaf.owner
                    .as_function()
                    .is_some_and(|(owner, _)| owner == &function.identity)
            })
            .collect();
        assert_eq!(leaves.len(), count, "{name}");
        for leaf in leaves {
            assert_eq!(leaf.binding.module, facts.module);
            assert_eq!(leaf.binding.definition_kind, DefinitionKind::Assignment);
            assert_eq!(leaf.binding_scope, facts.module_body_identity());
            assert_eq!(leaf.class.definition.module, target.module);
            assert_eq!(leaf.class.source_digest, target.source_digest);
            assert_ne!(leaf.binding, leaf.class.definition);
            let import_text = match leaf.name.as_str() {
                "Box" => "Box",
                "Alias" => "Box as Alias",
                name => panic!("unexpected imported leaf {name}"),
            };
            let import_start = source
                .find(&format!("from targets import {import_text}\n"))
                .unwrap()
                + "from targets import ".len();
            assert_eq!(leaf.binding.source_range.start as usize, import_start);
            assert_eq!(
                leaf.binding.source_range.end as usize,
                import_start + import_text.len()
            );
            assert!(facts.global_bindings.iter().any(|global| {
                global.name == leaf.name && global.definition.as_ref() == Some(&leaf.binding)
            }));
        }
    }
    fs::write(
        fixture.project.join("targets.py"),
        format!("{target_source}# different dependency source bytes\n"),
    )?;
    assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
    let (second, updated) = fixture.run()?;
    assert!(!target_marker.exists());
    assert_ne!(first.generation, second.generation);
    let updated = fixture.facts(&updated)?;
    assert_eq!(updated.nominal_bindings.len(), facts.nominal_bindings.len());
    for (before, after) in facts.nominal_bindings.iter().zip(&updated.nominal_bindings) {
        assert_eq!(before.binding, after.binding);
        assert_eq!(before.binding_scope, after.binding_scope);
        assert_ne!(before.class.source_digest, after.class.source_digest);
    }
    Ok(())
}

#[test]
fn offline_check_nominal_fields_keep_assignment_owners_and_inherited_dependencies() -> Result<()> {
    let fixture = Fixture::new("")?;
    let marker = fixture
        .directory
        .path()
        .join("field-source-was-executed.txt");
    let source = format!(
        r#"# soac: module(strict_assign=true, checked_attr=true)
from pathlib import Path
from dataclasses import dataclass
from bases import Base, Target
@dataclass
class Record(Base):
    payload: Target
def family():
    class LocalTarget:
        pass
    class Holder:
        payload: LocalTarget
        own: Holder | None
    class MethodHolder:
        def __init__(self, value):
            self.payload: LocalTarget = value
    def replace_target(value: type[LocalTarget]):
        nonlocal LocalTarget
        LocalTarget = value
    return LocalTarget, Holder, MethodHolder, replace_target
Path({}).write_text("executed")
"#,
        serde_json::to_string(&marker)?,
    );
    let base_source = "# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\nclass Target:\n    pass\n@dataclass\nclass Base:\n    inherited: Target\n";
    fs::write(fixture.project.join("model.py"), &source)?;
    fs::write(fixture.project.join("bases.py"), base_source)?;
    let (publication, deployment) = fixture.run()?;
    assert!(!marker.exists());
    let facts = fixture.facts(&deployment)?;
    let bases = fixture.module_facts(&deployment, "bases")?;
    assert_eq!(
        facts.schema_version,
        soac_contracts::ARTIFACT_SCHEMA_VERSION
    );
    let record = facts
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "Record")
        .unwrap();
    let base = bases
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "Base")
        .unwrap();
    let inherited = record
        .instance_fields
        .iter()
        .find(|field| field.name == "inherited")
        .unwrap();
    let base_field = base
        .instance_fields
        .iter()
        .find(|field| field.name == "inherited")
        .unwrap();
    assert_eq!(
        inherited.annotation_reference(),
        base_field.annotation_reference()
    );
    let inherited_reference = inherited.annotation_reference().unwrap();
    assert_eq!(
        inherited_reference.declaring_class.definition.module,
        bases.module
    );
    assert_eq!(
        inherited_reference.annotation_definition.module,
        bases.module
    );
    assert!(!facts.nominal_bindings.iter().any(|binding| matches!(&binding.owner, NominalBindingOwner::Field { field } if field == &inherited_reference)));
    assert_eq!(bases.nominal_bindings.iter().filter(|binding| matches!(&binding.owner, NominalBindingOwner::Field { field } if field == &inherited_reference)).count(), 1);
    for (class_name, field_name, binding_scope) in [
        ("Record", "payload", "<module>"),
        ("family.<locals>.Holder", "payload", "family"),
        ("family.<locals>.Holder", "own", "family"),
        ("family.<locals>.MethodHolder", "payload", "family"),
    ] {
        let class = facts
            .classes
            .iter()
            .find(|class| class.identity.lexical_qualname == class_name)
            .unwrap();
        let field = class
            .instance_fields
            .iter()
            .find(|field| field.name == field_name)
            .unwrap();
        let reference = field.annotation_reference().unwrap();
        assert_eq!(reference.declaring_class.definition, class.identity);
        assert_eq!(
            reference.annotation_definition.definition_kind,
            DefinitionKind::Assignment
        );
        let bindings: Vec<_> = facts.nominal_bindings.iter().filter(|binding| matches!(&binding.owner, NominalBindingOwner::Field { field } if field == &reference)).collect();
        assert_eq!(bindings.len(), 1, "{class_name}.{field_name}");
        assert_eq!(bindings[0].binding_scope.lexical_qualname, binding_scope);
        assert_eq!(
            &source[bindings[0].expression_range.start as usize
                ..bindings[0].expression_range.end as usize],
            bindings[0].name
        );
    }
    fs::write(
        fixture.project.join("bases.py"),
        format!("{base_source}# changed source identity\n"),
    )?;
    assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
    let (updated, updated_deployment) = fixture.run()?;
    assert_ne!(publication.generation, updated.generation);
    assert!(!marker.exists());
    let updated_facts = fixture.facts(&updated_deployment)?;
    let updated_record = updated_facts
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "Record")
        .unwrap();
    let updated_inherited = updated_record
        .instance_fields
        .iter()
        .find(|field| field.name == "inherited")
        .unwrap();
    assert_ne!(
        inherited.annotation_reference(),
        updated_inherited.annotation_reference()
    );
    Ok(())
}

#[test]
fn offline_check_tracks_primitive_imports_transitive_sources_and_new_shadow_stubs() -> Result<()> {
    let fixture = Fixture::new(
        "# soac: module(strict_assign=true, checked_attr=true)\nfrom configuration_values import NUMBER\nVALUE = NUMBER\n",
    )?;
    fs::write(
        fixture.project.join("configuration_values.py"),
        "from nested_values import NUMBER\n",
    )?;
    fs::write(fixture.project.join("nested_values.py"), "NUMBER = 42\n")?;
    let (first, deployment) = fixture.run()?;
    let facts = fixture.facts(&deployment)?;
    for name in ["configuration_values", "nested_values"] {
        assert!(
            facts
                .consumed_dependencies
                .iter()
                .any(|dependency| dependency.module.module_name == name)
        );
    }
    fs::write(fixture.project.join("nested_values.py"), "NUMBER = 43\n")?;
    assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
    let (second, updated) = fixture.run()?;
    assert_ne!(second.generation, first.generation);
    fs::write(fixture.project.join("nested_values.pyi"), "NUMBER: int\n")?;
    assert!(verify_analysis_inputs(&updated.analysis_inputs).is_err());
    let (third, _) = fixture.run()?;
    assert_ne!(third.generation, second.generation);
    Ok(())
}

#[test]
fn offline_check_fingerprints_actual_configuration_and_source_rules() -> Result<()> {
    let source =
        "# soac: module(strict_assign=true, checked_attr=true)\nclass Value:\n    value: int\n";
    let fixture = Fixture::new(source)?;
    let config_path = fixture.project.join("pyproject.toml");
    let base = fs::read_to_string(&config_path)?;
    let (first, deployment) = fixture.run()?;
    fs::write(
        &config_path,
        format!("{base}\n[tool.ty.analysis]\nstrict-equality-semantics=false\n"),
    )?;
    assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
    let (configured, configured_deployment) = fixture.run()?;
    assert_ne!(configured.generation, first.generation);
    fs::write(
        fixture.project.join("model.py"),
        source.replace("checked_attr=true", "checked_attr=false"),
    )?;
    assert!(verify_analysis_inputs(&configured_deployment.analysis_inputs).is_err());
    let (overridden, updated) = fixture.run()?;
    assert_ne!(overridden.generation, configured.generation);
    let facts = fixture.facts(&updated)?;
    assert!(facts.language_policy.strict_assign);
    assert!(!facts.language_policy.checked_attr);
    assert_eq!(
        facts
            .language_policy
            .checked_fields(facts.classes[0].identity.source_range),
        CheckedFieldPolicy::Disabled
    );
    assert!(
        matches!(&facts.classes[0].participation, ParticipationProposal::Dynamic(reasons)
        if reasons.contains(&DynamicClassReason::PolicyOptOut))
    );
    assert_ne!(
        updated.environment.normalized_project_policy,
        configured_deployment.environment.normalized_project_policy
    );
    assert!(updated.environment.analysis.strict_equality_semantics);
    Ok(())
}

#[test]
fn offline_check_unselected_and_future_only_sources_publish_no_contracts() -> Result<()> {
    for source in [
        "class Ordinary:\n    value: int\nraise RuntimeError('offline source must not execute')\n",
        "from __future__ import strict\nclass Ordinary:\n    value: int\nraise RuntimeError('future import is not source selection')\n",
        "'# soac: module(strict_assign=true, checked_attr=true)'\nclass Ordinary:\n    value: int\n",
        "value: int = 'ordinary annotation mismatch'\n",
    ] {
        let fixture = Fixture::new(source)?;
        let (publication, deployment) = fixture.run()?;
        assert_eq!(publication.modules, 0);
        assert!(deployment.modules.is_empty());
        assert!(fixture.manifest(&deployment)?.manifest().modules.is_empty());
        let path = fixture.project.join("model.py").canonicalize()?;
        assert!(deployment.analysis_inputs.iter().any(|input| {
            input.path == path
                && matches!(&input.state, AnalysisInputState::File { digest, .. }
                    if *digest == Fingerprint::digest(source))
        }));
        assert_eq!(fs::read_to_string(path)?, source);
    }
    Ok(())
}

#[test]
fn offline_check_ordinary_errors_do_not_block_selected_consumers() -> Result<()> {
    let fixture = Fixture::new(
        "# soac: module(checked_attr=true)\nfrom ordinary import value\nclass Checked:\n    field: int\ndef supplied(): return value\n",
    )?;
    let ordinary_path = fixture.project.join("ordinary.py");
    let ordinary_source = "value: int = 'ordinary annotation mismatch'\n";
    fs::write(&ordinary_path, ordinary_source)?;
    let (_, deployment) = fixture.run()?;
    assert_eq!(
        deployment
            .modules
            .iter()
            .map(|module| module.module_name.as_str())
            .collect::<Vec<_>>(),
        ["model"]
    );
    let facts = fixture.facts(&deployment)?;
    let dependency = facts
        .consumed_dependencies
        .iter()
        .find(|dependency| dependency.module.module_name == "ordinary")
        .expect("the ordinary producer is a consumed source, not a signed module");
    assert_eq!(
        dependency.source_digest,
        Fingerprint::digest(ordinary_source)
    );
    assert!(dependency.strict_policy.is_none());

    let original = fs::read(fixture.project.join("deployment.json"))?;
    let changed_source = "value: int = 'changed ordinary annotation mismatch'\n";
    fs::write(&ordinary_path, changed_source)?;
    assert!(deployment.verified_analysis_dependencies("model").is_err());

    // Selecting the same erroneous source requires a valid contract export;
    // this is a checker error, not a runtime annotation obligation.
    fs::write(
        &ordinary_path,
        format!("# soac: module(strict_assign=true)\n{changed_source}"),
    )?;
    let error = check(fixture.options()).expect_err("selected type errors block publication");
    assert!(matches!(
        error.downcast_ref::<soac_contracts::ContractError>(),
        Some(soac_contracts::ContractError::BlockingDiagnostic(_))
    ));
    assert_eq!(fs::read(fixture.project.join("deployment.json"))?, original);
    Ok(())
}

#[test]
fn offline_check_mutable_module_can_request_checked_classes() -> Result<()> {
    let source = "# soac: module(checked_attr=true)\nLIMIT = 1\nclass Checked:\n    value: int\ndef change():\n    globals()['LIMIT'] = 2\n";
    let fixture = Fixture::new(source)?;
    let (_, deployment) = fixture.run()?;
    let facts = fixture.facts(&deployment)?;
    assert!(!facts.language_policy.strict_assign);
    assert!(facts.language_policy.checked_attr);
    assert!(
        facts
            .global_bindings
            .iter()
            .all(|binding| binding.mutability == GlobalMutability::Unknown)
    );
    let class = &facts.classes[0];
    assert_eq!(class.participation, ParticipationProposal::Candidate);
    assert_eq!(
        class.required_field_bindings(&facts.language_policy).len(),
        1
    );
    assert_eq!(facts.source_digest, Fingerprint::digest(source));
    Ok(())
}

#[test]
fn offline_check_class_only_rule_does_not_select_other_or_nested_classes() -> Result<()> {
    let source = "\"\"\"Original λ source bytes\"\"\"\n# soac: class(checked_attr=true)\nclass Checked:\n    value: int\n    class Nested:\n        nested: int\nclass Ordinary:\n    value: int\nLIMIT = 1\ndef change():\n    globals()['LIMIT'] = 2\n";
    let fixture = Fixture::new(source)?;
    let (_, deployment) = fixture.run()?;
    let facts = fixture.facts(&deployment)?;
    assert!(!facts.language_policy.strict_assign && !facts.language_policy.checked_attr);
    assert_eq!(facts.language_policy.class_overrides.len(), 1);
    assert!(
        facts
            .global_bindings
            .iter()
            .all(|binding| binding.mutability == GlobalMutability::Unknown)
    );
    let checked = facts
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "Checked")
        .unwrap();
    assert_eq!(checked.participation, ParticipationProposal::Candidate);
    assert_eq!(
        checked
            .required_field_bindings(&facts.language_policy)
            .len(),
        1
    );
    assert_eq!(
        facts.language_policy.class_overrides[0].class_range,
        checked.identity.source_range
    );
    for name in ["Checked.Nested", "Ordinary"] {
        let class = facts
            .classes
            .iter()
            .find(|class| class.identity.lexical_qualname == name)
            .unwrap();
        assert!(
            matches!(&class.participation, ParticipationProposal::Dynamic(reasons)
            if reasons.contains(&DynamicClassReason::PolicyOptOut))
        );
        assert!(
            class
                .required_field_bindings(&facts.language_policy)
                .is_empty()
        );
    }
    assert_eq!(facts.source_digest, Fingerprint::digest(source));
    assert_eq!(
        fs::read_to_string(fixture.project.join("model.py"))?,
        source
    );
    Ok(())
}

#[test]
fn offline_check_class_rules_bind_repeated_nested_and_decorated_source_ranges() -> Result<()> {
    let source = "# soac: module(checked_attr=true)\nfrom dataclasses import dataclass\n# soac: class(checked_attr=false)\n@dataclass\nclass Repeated:\n    first: int = 1\nclass Repeated:\n    second: int = 2\n# soac: class(checked_attr=false)\nclass Outer:\n    class Inner:\n        value: int\n";
    let fixture = Fixture::new(source)?;
    let (_, deployment) = fixture.run()?;
    let facts = fixture.facts(&deployment)?;
    assert_eq!(facts.language_policy.class_overrides.len(), 2);
    let first = facts
        .classes
        .iter()
        .find(|class| {
            class.identity.lexical_qualname == "Repeated"
                && class
                    .instance_fields
                    .iter()
                    .any(|field| field.name == "first")
        })
        .unwrap();
    let second = facts
        .classes
        .iter()
        .find(|class| {
            class.identity.lexical_qualname == "Repeated"
                && class
                    .instance_fields
                    .iter()
                    .any(|field| field.name == "second")
        })
        .unwrap();
    let outer = facts
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "Outer")
        .unwrap();
    let inner = facts
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "Outer.Inner")
        .unwrap();
    assert_eq!(
        first.identity.source_range.start as usize,
        source.find("@dataclass").unwrap()
    );
    assert_eq!(
        first.identity.source_range.end as usize,
        source.find("    first: int = 1").unwrap() + "    first: int = 1".len()
    );
    assert_ne!(first.identity.source_range, second.identity.source_range);
    for class in [first, outer] {
        assert!(
            !facts
                .language_policy
                .checked_attributes(class.identity.source_range)
        );
        assert!(
            matches!(&class.participation, ParticipationProposal::Dynamic(reasons)
            if reasons.contains(&DynamicClassReason::PolicyOptOut))
        );
    }
    for class in [second, inner] {
        assert!(
            facts
                .language_policy
                .checked_attributes(class.identity.source_range)
        );
        assert_eq!(class.participation, ParticipationProposal::Candidate);
    }
    assert_eq!(facts.source_digest, Fingerprint::digest(source));
    Ok(())
}

#[test]
fn offline_check_package_rules_authenticate_absence_and_do_not_inherit_module_overrides()
-> Result<()> {
    let fixture = Fixture::new(
        "# soac: module(strict_assign=true, checked_attr=true)\nfrom pkg.model import Value\ndef identity(value: Value) -> Value: return value\n",
    )?;
    fs::create_dir(fixture.project.join("pkg"))?;
    let child_source = "class Value:\n    value: int\n";
    fs::write(fixture.project.join("pkg/model.py"), child_source)?;
    let package_path = fixture.project.join("pkg/__init__.py");
    let (initial, absent) = fixture.run()?;
    assert!(
        absent
            .modules
            .iter()
            .all(|module| module.module_name != "pkg.model")
    );
    assert!(absent.analysis_inputs.iter().any(|input| {
        input.path == package_path && matches!(&input.state, AnalysisInputState::Missing)
    }));
    let marker = fixture.project.join("package-source-was-imported.txt");
    let package_source = format!(
        "# soac: package(strict_assign=true, checked_attr=true)\n# soac: module(strict_assign=false, checked_attr=true)\nfrom pathlib import Path\nPath({}).write_text('imported')\n",
        serde_json::to_string(&marker)?,
    );
    fs::write(&package_path, &package_source)?;
    assert!(verify_analysis_inputs(&absent.analysis_inputs).is_err());
    let (selected, deployment) = fixture.run()?;
    assert_ne!(selected.generation, initial.generation);
    assert!(
        !marker.exists(),
        "package policy discovery must not execute __init__.py"
    );
    let package = fixture.module_facts(&deployment, "pkg")?;
    let child = fixture.module_facts(&deployment, "pkg.model")?;
    assert!(!package.language_policy.strict_assign);
    assert!(package.language_policy.checked_attr);
    assert!(child.language_policy.strict_assign && child.language_policy.checked_attr);
    assert_eq!(
        child.classes[0].participation,
        ParticipationProposal::Candidate
    );
    assert!(deployment.analysis_inputs.iter().any(|input| {
        input.path == package_path
            && matches!(&input.state, AnalysisInputState::File { digest, .. }
                if *digest == Fingerprint::digest(&package_source))
    }));
    let changed_source = package_source.replacen(
        "package(strict_assign=true, checked_attr=true)",
        "package(strict_assign=true, checked_attr=false)",
        1,
    );
    fs::write(&package_path, changed_source)?;
    assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
    let (changed, updated) = fixture.run()?;
    assert_ne!(changed.generation, selected.generation);
    assert!(!marker.exists());
    let updated_child = fixture.module_facts(&updated, "pkg.model")?;
    assert_eq!(updated_child.module, child.module);
    assert!(updated_child.language_policy.strict_assign);
    assert!(!updated_child.language_policy.checked_attr);
    assert!(
        matches!(&updated_child.classes[0].participation, ParticipationProposal::Dynamic(reasons)
        if reasons.contains(&DynamicClassReason::PolicyOptOut))
    );
    assert!(
        fixture
            .module_facts(&updated, "pkg")?
            .language_policy
            .checked_attr
    );
    assert_ne!(
        updated.environment.normalized_project_policy,
        deployment.environment.normalized_project_policy
    );
    Ok(())
}

#[test]
fn offline_check_rejects_retired_config_policy_without_replacing_authority() -> Result<()> {
    let fixture =
        Fixture::new("# soac: module(checked_attr=true)\nclass Value:\n    value: int\n")?;
    let (_, deployment) = fixture.run()?;
    let original = fs::read(fixture.project.join("deployment.json"))?;
    let config = fixture.project.join("pyproject.toml");
    let base = fs::read_to_string(&config)?;
    for retired in [
        "[tool.soac.strict]\ninclude=['*.py']\n",
        "[[tool.soac.strict.overrides]]\ninclude=['model.py']\nchecked_fields='supported_annotations'\n",
    ] {
        fs::write(&config, format!("{base}\n{retired}"))?;
        assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
        assert!(check(fixture.options()).is_err());
        assert_eq!(fs::read(fixture.project.join("deployment.json"))?, original);
    }
    Ok(())
}

#[test]
fn offline_check_rejects_retired_discovered_parent_config_without_replacing_authority() -> Result<()>
{
    let fixture =
        Fixture::new("# soac: module(checked_attr=true)\nclass Value:\n    value: int\n")?;
    let source_root = fixture.project.join("src");
    fs::create_dir(&source_root)?;
    fs::rename(
        fixture.project.join("model.py"),
        source_root.join("model.py"),
    )?;
    let config = fixture.project.join("pyproject.toml");
    let base = format!(
        "{}\n[tool.ty.analysis]\nstrict-equality-semantics=false\n",
        fs::read_to_string(&config)?
    );
    fs::write(&config, &base)?;
    // This higher ancestor is not selected: the nearer actual ty config wins.
    fs::write(
        fixture.directory.path().join("pyproject.toml"),
        "[tool.soac.strict]\n",
    )?;
    let options = || Check {
        project: source_root.clone(),
        modules: vec!["model=model.py".into()],
        output: fixture.project.join("artifacts"),
        signing_key: fixture.project.join("signing.key"),
        deployment: fixture.project.join("deployment.json"),
        ..fixture.options()
    };
    let (publication, deployment) = fixture.run_with_options(options())?;
    assert_eq!(publication.modules, 1);
    let observed_config = config.canonicalize()?;
    assert!(deployment.analysis_inputs.iter().any(|input| {
        input.path == observed_config
            && matches!(&input.state, AnalysisInputState::File { digest, .. }
                if *digest == Fingerprint::digest(&base))
    }));
    let original = fs::read(fixture.project.join("deployment.json"))?;
    for retired in [
        "[tool.soac.strict]\ninclude=['*.py']\n",
        "[[tool.soac.strict.overrides]]\ninclude=['model.py']\nchecked_fields='supported_annotations'\n",
    ] {
        fs::write(&config, format!("{base}\n{retired}"))?;
        assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
        let error = check(options())
            .expect_err("the actual discovered project cannot retain retired SOAC configuration");
        assert!(
            error
                .root_cause()
                .to_string()
                .contains("[tool.soac.strict]"),
            "{error:#}"
        );
        assert_eq!(fs::read(fixture.project.join("deployment.json"))?, original);
    }
    Ok(())
}

#[test]
fn offline_check_suppressed_errors_demote_only_affected_classes() -> Result<()> {
    let fixture = Fixture::new(
        "# soac: module(strict_assign=true, checked_attr=true)\nclass Damaged:\n    value: int = 'wrong'  # ty: ignore[invalid-assignment]\nclass Fine:\n    value: int = 1\n",
    )?;
    let (_, deployment) = fixture.run()?;
    let facts = fixture.facts(&deployment)?;
    let damaged = facts
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "Damaged")
        .unwrap();
    let fine = facts
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "Fine")
        .unwrap();
    assert!(
        matches!(&damaged.participation, ParticipationProposal::Dynamic(reasons) if reasons.contains(&DynamicClassReason::IgnoredDiagnostic))
    );
    assert_eq!(fine.participation, ParticipationProposal::Candidate);
    Ok(())
}

#[test]
fn offline_check_authenticates_external_strict_base_proposals_and_invalidation() -> Result<()> {
    let fixture = Fixture::new(
        "# soac: module(strict_assign=true, checked_attr=true)\nfrom bridge import Middle\nclass Child(Middle):\n    def __init__(self):\n        super().__init__()\n        self.own: int = 3\n",
    )?;
    let base_source = "# soac: module(checked_attr=true)\nclass Base:\n    def __init__(self):\n        self.inherited: int = 1\n";
    fs::write(fixture.project.join("base.py"), base_source)?;
    fs::write(
        fixture.project.join("bridge.py"),
        "# soac: module(strict_assign=true, checked_attr=true)\nfrom base import Base\nclass Middle(Base): pass\n",
    )?;
    let (first, deployment) = fixture.run()?;
    let facts = fixture.facts(&deployment)?;
    let child = &facts.classes[0];
    assert_eq!(child.participation, ParticipationProposal::Candidate);
    assert_eq!(
        facts
            .language_policy
            .checked_fields(child.identity.source_range),
        CheckedFieldPolicy::SupportedAnnotations
    );
    let ancestor = child
        .inheritance
        .linearized_bases
        .iter()
        .filter_map(soac_contracts::BaseReference::as_class)
        .find(|base| base.definition.module.module_name == "base")
        .unwrap();
    assert_eq!(ancestor.source_digest, Fingerprint::digest(base_source));
    let base_policy = &deployment
        .modules
        .iter()
        .find(|module| module.module_name == "base")
        .unwrap()
        .policy;
    assert!(!base_policy.strict_assign);
    assert_eq!(
        base_policy.checked_fields(ancestor.definition.source_range),
        CheckedFieldPolicy::SupportedAnnotations
    );
    let dependency = facts
        .consumed_dependencies
        .iter()
        .find(|dependency| dependency.module == ancestor.definition.module)
        .unwrap();
    assert_eq!(dependency.strict_policy, Some(base_policy.fingerprint()?));
    assert_eq!(dependency.source_digest, ancestor.source_digest);
    assert!(!facts.function_has_statically_dynamic_class_owner(
        &child.methods[0].implementation.clone().unwrap()
    ));

    let changed = "# soac: module(checked_attr=true)\ndef framework[T](value: T) -> T: return value\n@framework\nclass Base:\n    def __init__(self):\n        self.inherited: int = 1\n";
    fs::write(fixture.project.join("base.py"), changed)?;
    assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
    let (second, updated) = fixture.run()?;
    assert_ne!(second.generation, first.generation);
    let updated_facts = fixture.facts(&updated)?;
    let updated_child = &updated_facts.classes[0];
    assert!(
        matches!(&updated_child.participation, ParticipationProposal::Dynamic(reasons)
        if reasons.contains(&DynamicClassReason::MutableBase))
    );
    assert!(updated_facts.function_has_statically_dynamic_class_owner(
        &updated_child.methods[0].implementation.clone().unwrap()
    ));
    assert!(
        updated_facts
            .consumed_dependencies
            .iter()
            .any(|dependency| dependency.module.module_name == "base"
                && dependency.source_digest == Fingerprint::digest(changed))
    );
    Ok(())
}

#[test]
fn offline_check_rejects_real_strict_errors_without_replacing_startup_authority() -> Result<()> {
    let fixture =
        Fixture::new("# soac: module(strict_assign=true, checked_attr=true)\nclass Value: pass\n")?;
    fixture.run()?;
    let original = fs::read(fixture.project.join("deployment.json"))?;
    fs::write(
        fixture.project.join("model.py"),
        "# soac: module(strict_assign=true, checked_attr=true)\nLIMIT=1\ndef invalid(): globals()['LIMIT']=2\n",
    )?;
    assert!(check(fixture.options()).is_err());
    assert_eq!(fs::read(fixture.project.join("deployment.json"))?, original);
    Ok(())
}

#[test]
fn offline_check_and_loader_reject_tampered_artifacts() -> Result<()> {
    let fixture =
        Fixture::new("# soac: module(strict_assign=true, checked_attr=true)\nclass Value: pass\n")?;
    let (_, deployment) = fixture.run()?;
    let manifest = fixture.manifest(&deployment)?;
    let index = manifest.module_index("model")?;
    let shard = deployment
        .artifact_directory
        .join("modules")
        .join(format!("{}.soac-types", index.shard_digest));
    fs::write(&shard, b"tampered signed shard")?;
    assert!(
        verify_complete_generation(manifest, |digest| fs::read(
            deployment
                .artifact_directory
                .join("modules")
                .join(format!("{digest}.soac-types"))
        )
        .map_err(|error| soac_contracts::ContractError::InvalidStructure(error.to_string())))
        .is_err()
    );
    assert!(check(fixture.options()).is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn offline_check_detects_same_byte_dependency_symlink_retargeting() -> Result<()> {
    let fixture = Fixture::new(
        "# soac: module(strict_assign=true, checked_attr=true)\nfrom external import NUMBER\nVALUE = NUMBER\n",
    )?;
    let first = fixture.directory.path().join("first.py");
    let second = fixture.directory.path().join("second.py");
    fs::write(&first, "NUMBER = 42\n")?;
    fs::write(&second, "NUMBER = 42\n")?;
    let alias = fixture.project.join("external.py");
    std::os::unix::fs::symlink(&first, &alias)?;
    let (_, deployment) = fixture.run()?;
    fs::remove_file(&alias)?;
    std::os::unix::fs::symlink(&second, &alias)?;
    assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn offline_check_binds_actual_cpython_library_bytes_without_changing_shared_build() -> Result<()> {
    let mut fixture =
        Fixture::new("# soac: module(strict_assign=true, checked_attr=true)\nclass Value: pass\n")?;
    let libraries = fixture.use_private_interpreter()?;
    let (first, deployment) = fixture.run()?;
    OpenOptions::new()
        .append(true)
        .open(&libraries[0])?
        .write_all(b"\0SOAC-private-ABI-test")?;
    assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
    let (changed, updated) = fixture.run()?;
    assert_ne!(changed.generation, first.generation);
    assert_ne!(
        updated.environment.cpython_abi_fingerprint,
        deployment.environment.cpython_abi_fingerprint
    );
    assert_eq!(changed.reused_shards, 1);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn offline_check_preserves_selected_venv_and_package_only_dependencies() -> Result<()> {
    let mut fixture = Fixture::new(
        "# soac: module(strict_assign=true, checked_attr=true)\nfrom only_this_venv import ANSWER\ndef read() -> int:\n    return ANSWER\n",
    )?;
    let base = fixture.python.canonicalize()?;
    let venv = fixture.directory.path().join("selected-venv");
    let output = Command::new(&base)
        .args(["-I", "-B", "-m", "venv", "--without-pip", "--symlinks"])
        .arg(&venv)
        .output()?;
    ensure!(
        output.status.success(),
        "real selected venv creation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fixture.python = venv.join("bin/python");
    let selected = interpreter_identity(&fixture.python)?;
    assert_eq!(Path::new(&selected.prefix), venv);
    let site = Path::new(&selected.site_packages[0]);
    let package = site.join("only_this_venv");
    fs::create_dir_all(&package)?;
    fs::write(package.join("__init__.pyi"), "ANSWER: int\n")?;
    fs::write(
        package.join("__init__.py"),
        "raise AssertionError('offline checker imported the selected venv package')\n",
    )?;
    let (publication, deployment) = fixture.run()?;
    assert_eq!(Path::new(&deployment.target_interpreter.prefix), venv);
    assert_eq!(Path::new(&deployment.target_interpreter.executable), base);
    assert!(
        deployment
            .target_interpreter
            .site_packages
            .iter()
            .all(|path| Path::new(path).starts_with(&venv))
    );
    assert!(
        deployment
            .analysis_inputs
            .iter()
            .any(|input| input.path == fixture.python)
    );
    assert!(
        deployment
            .analysis_inputs
            .iter()
            .any(|input| input.path == venv.join("pyvenv.cfg"))
    );
    let facts = fixture.facts(&deployment)?;
    assert!(
        facts
            .consumed_dependencies
            .iter()
            .any(|dependency| dependency.module.module_name == "only_this_venv")
    );
    assert!(
        deployment
            .analysis_inputs
            .iter()
            .any(|input| input.path == package.join("__init__.pyi"))
    );

    let config = venv.join("pyvenv.cfg");
    let mut contents = fs::read_to_string(&config)?;
    contents.push_str("\n# changed selected-venv configuration\n");
    fs::write(config, contents)?;
    assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
    let (changed, fresh) = fixture.run()?;
    assert_ne!(publication.generation, changed.generation);
    fs::write(package.join("__init__.pyi"), "ANSWER: str\n")?;
    assert!(verify_analysis_inputs(&fresh.analysis_inputs).is_err());
    assert!(fixture.run().is_err());
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn offline_check_tracks_new_path_files_and_distribution_metadata() -> Result<()> {
    let mut fixture =
        Fixture::new("# soac: module(strict_assign=true, checked_attr=true)\nclass Value: pass\n")?;
    fixture.use_private_interpreter()?;
    let site = PathBuf::from(&interpreter_identity(&fixture.python)?.site_packages[0]);
    let (first, deployment) = fixture.run()?;
    fs::write(site.join("new.pth"), "# new resolver input\n")?;
    assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
    let (with_path, after_path) = fixture.run()?;
    assert_ne!(with_path.generation, first.generation);
    let metadata = site.join("fixture-1.0.dist-info");
    fs::create_dir(&metadata)?;
    fs::write(metadata.join("METADATA"), "Name: fixture\nVersion: 1.0\n")?;
    assert!(verify_analysis_inputs(&after_path.analysis_inputs).is_err());
    let (_, after_metadata) = fixture.run()?;
    fs::write(metadata.join("METADATA"), "Name: fixture\nVersion: 1.1\n")?;
    assert!(verify_analysis_inputs(&after_metadata.analysis_inputs).is_err());
    Ok(())
}

#[test]
fn offline_check_resolves_nested_class_cells_without_import_or_global_guessing() -> Result<()> {
    let fixture = Fixture::new("")?;
    let marker = fixture.project.join("nested-class-source-was-imported.txt");
    let mut source = r#"# soac: module(strict_assign=true, checked_attr=true)
from pathlib import Path
def exercise():
    class C:
        def method(self):
            def nested():
                return __class__
            return nested()
    return C().method(), C
"#
    .to_owned();
    source.push_str(&format!(
        "Path({}).write_text('imported')\n",
        serde_json::to_string(&marker)?
    ));
    fs::write(fixture.project.join("model.py"), &source)?;
    let (_, deployment) = fixture.run()?;
    assert!(!marker.exists(), "offline analysis must not execute source");
    let facts = fixture.facts(&deployment)?;
    let class = facts
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "exercise.<locals>.C")
        .unwrap();
    assert_eq!(class.participation, ParticipationProposal::Candidate);
    let nested = facts
        .functions
        .iter()
        .find(|function| {
            function.identity.lexical_qualname == "exercise.<locals>.C.method.<locals>.nested"
        })
        .unwrap();
    assert_eq!(nested.identity.definition_kind, DefinitionKind::Function);
    assert_eq!(
        nested.signature.return_annotation_origin,
        AnnotationOrigin::Absent
    );

    let original_deployment = fs::read(fixture.project.join("deployment.json"))?;
    fs::write(
        fixture.project.join("model.py"),
        "# soac: module(strict_assign=true, checked_attr=true)\nclass C:\n    def method(self):\n        global __class__\n        def nested(): return __class__\n        return nested\n",
    )?;
    assert!(check(fixture.options()).is_err());
    assert_eq!(
        fs::read(fixture.project.join("deployment.json"))?,
        original_deployment
    );
    Ok(())
}

#[test]
fn offline_check_tracks_nonlocal_class_cells_without_namespace_bindings() -> Result<()> {
    let fixture = Fixture::new("")?;
    let marker = fixture
        .project
        .join("nonlocal-class-source-was-imported.txt");
    let mut source = r#"# soac: module(strict_assign=true, checked_attr=true)
from pathlib import Path
def factory():
    class Model:
        class Inner:
            nonlocal __class__
            __class__ = "construction"
            saved: str = __class__
            def own_class(self):
                return __class__
        def reader(self):
            def read():
                nonlocal __class__
                return __class__
            return read
        def replace(self, value):
            nonlocal __class__
            __class__ = value
        def erase(self):
            nonlocal __class__
            del __class__
    return Model
"#
    .to_owned();
    source.push_str(&format!(
        "Path({}).write_text('imported')\n",
        serde_json::to_string(&marker)?
    ));
    fs::write(fixture.project.join("model.py"), &source)?;
    let (_, deployment) = fixture.run()?;
    assert!(!marker.exists(), "offline analysis must not execute source");
    let facts = fixture.facts(&deployment)?;
    assert!(
        facts
            .global_bindings
            .iter()
            .all(|binding| binding.name != "__class__")
    );
    let model = facts
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "factory.<locals>.Model")
        .unwrap();
    assert_eq!(model.participation, ParticipationProposal::Candidate);
    assert!(
        model
            .class_members
            .iter()
            .all(|member| member.name != "__class__")
    );
    let inner = facts
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "factory.<locals>.Model.Inner")
        .unwrap();
    assert!(
        inner
            .class_members
            .iter()
            .all(|member| member.name != "__class__")
    );
    assert!(
        inner
            .instance_fields
            .iter()
            .any(|field| field.name == "saved")
    );
    assert!(
        facts
            .functions
            .iter()
            .any(|function| function.identity.lexical_qualname
                == "factory.<locals>.Model.Inner.own_class")
    );
    let read = facts
        .functions
        .iter()
        .find(|function| {
            function.identity.lexical_qualname == "factory.<locals>.Model.reader.<locals>.read"
        })
        .unwrap();
    assert_eq!(read.identity.definition_kind, DefinitionKind::Function);
    assert_eq!(
        read.signature.return_annotation_origin,
        AnnotationOrigin::Absent
    );

    let original_deployment = fs::read(fixture.project.join("deployment.json"))?;
    fs::write(
        fixture.project.join("model.py"),
        "# soac: module(strict_assign=true, checked_attr=true)\nclass Model:\n    def method(self):\n        __class__: int = 1\n        def invalid():\n            nonlocal __class__\n            __class__ = 'wrong'\n        return invalid\n",
    )?;
    assert!(check(fixture.options()).is_err());
    assert_eq!(
        fs::read(fixture.project.join("deployment.json"))?,
        original_deployment
    );
    fs::write(
        fixture.project.join("model.py"),
        "# soac: module(strict_assign=true, checked_attr=true)\n__class__ = 100\nclass Outer:\n    class Inner:\n        nonlocal __class__\n        value = __class__\n",
    )?;
    assert!(check(fixture.options()).is_err());
    assert_eq!(
        fs::read(fixture.project.join("deployment.json"))?,
        original_deployment
    );
    Ok(())
}

#[test]
fn offline_check_excludes_only_semantic_dataclass_kw_only_markers() -> Result<()> {
    let fixture = Fixture::new("")?;
    let marker = fixture.project.join("kw-only-source-was-imported.txt");
    let mut source = r#"# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass, KW_ONLY as Marker
from pathlib import Path
from typing import ClassVar
@dataclass(init=False)
class Record:
    first: int = 1
    shared: ClassVar[int] = 2
    delimiter: Marker
    after: str = "value"
class Plain:
    delimiter: Marker
class KW_ONLY:
    pass
@dataclass
class Namesake:
    real: KW_ONLY
"#
    .to_owned();
    source.push_str(&format!(
        "Path({}).write_text('imported')\n",
        serde_json::to_string(&marker)?
    ));
    fs::write(fixture.project.join("model.py"), &source)?;
    let (_, deployment) = fixture.run()?;
    assert!(!marker.exists(), "offline analysis must not execute source");
    let facts = fixture.facts(&deployment)?;
    let class = |name| {
        facts
            .classes
            .iter()
            .find(|class| class.identity.lexical_qualname == name)
            .unwrap()
    };
    let record = class("Record");
    assert!(
        !record
            .transform
            .as_ref()
            .unwrap()
            .dataclass_options
            .as_ref()
            .unwrap()
            .init
    );
    assert!(
        record
            .instance_fields
            .iter()
            .all(|field| field.name != "delimiter")
    );
    assert_eq!(
        record
            .instance_fields
            .iter()
            .filter(|field| field.field_kind != soac_contracts::FieldKind::ClassVariable)
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        ["first", "after"]
    );
    assert!(
        class("Plain")
            .instance_fields
            .iter()
            .any(|field| field.name == "delimiter")
    );
    let actual = &class("Namesake").instance_fields[0];
    assert_eq!(actual.name, "real");
    let StaticType::NominalClass(reference) = &actual.value_type else {
        panic!("a user class named KW_ONLY is not a dataclass marker");
    };
    assert_eq!(reference.definition.lexical_qualname, "KW_ONLY");
    Ok(())
}

#[test]
fn offline_check_authenticates_semantic_builtin_bases_without_import_or_name_guesses() -> Result<()>
{
    use soac_contracts::{BaseReference, BuiltinType, ClassReference};

    let fixture = Fixture::new("")?;
    let marker = fixture.project.join("builtin-base-source-was-imported.txt");
    let mut source = r#"# soac: module(strict_assign=true, checked_attr=true)
from builtins import object as ObjectRoot
from dependency import Root
from pathlib import Path
class Implicit:
    pass
class Direct(object):
    pass
class Aliased(ObjectRoot):
    pass
class Imported(Root):
    pass
class Integer(int):
    pass
def namesake():
    class object:
        pass
    class Child(object):
        pass
    return Child
"#
    .to_owned();
    source.push_str(&format!(
        "Path({}).write_text('imported')\n",
        serde_json::to_string(&marker)?
    ));
    fs::write(fixture.project.join("model.py"), &source)?;
    fs::write(
        fixture.project.join("dependency.py"),
        "# soac: module(strict_assign=true, checked_attr=true)\nfrom builtins import object as Root\n",
    )?;
    let (first, deployment) = fixture.run()?;
    assert!(!marker.exists(), "offline analysis must not execute source");
    let facts = fixture.facts(&deployment)?;
    let class = |name| {
        facts
            .classes
            .iter()
            .find(|class| class.identity.lexical_qualname == name)
            .unwrap()
    };
    let object = BaseReference::Builtin(BuiltinType::Object);
    assert!(class("Implicit").bases.is_empty());
    assert_eq!(
        class("Implicit").inheritance.linearized_bases,
        [object.clone()]
    );
    for name in ["Direct", "Aliased", "Imported"] {
        let actual = class(name);
        assert_eq!(actual.bases, [object.clone()]);
        assert_eq!(actual.inheritance.linearized_bases, [object.clone()]);
        assert_eq!(actual.participation, ParticipationProposal::Candidate);
    }
    assert_eq!(
        class("Integer").bases,
        [BaseReference::Builtin(BuiltinType::Int)]
    );
    assert!(matches!(
        class("Integer").participation,
        ParticipationProposal::Dynamic(_)
    ));
    let shadow = ClassReference {
        definition: class("namesake.<locals>.object").identity.clone(),
        source_digest: facts.source_digest,
    };
    assert_eq!(
        class("namesake.<locals>.Child").bases,
        [BaseReference::Class(shadow.clone())]
    );
    assert_eq!(
        class("namesake.<locals>.Child")
            .inheritance
            .linearized_bases,
        [BaseReference::Class(shadow), object]
    );

    let changed = "# soac: module(strict_assign=true, checked_attr=true)\nclass Root:\n    pass\n";
    fs::write(fixture.project.join("dependency.py"), changed)?;
    assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
    let (second, updated) = fixture.run()?;
    assert_ne!(first.generation, second.generation);
    assert!(!marker.exists(), "reanalysis must not execute source");
    let updated_facts = fixture.facts(&updated)?;
    let imported = updated_facts
        .classes
        .iter()
        .find(|class| class.identity.lexical_qualname == "Imported")
        .unwrap();
    let [BaseReference::Class(base)] = imported.bases.as_slice() else {
        panic!("a source class replacing a builtin alias needs its own source reference");
    };
    assert_eq!(base.definition.module.module_name, "dependency");
    assert_eq!(base.definition.lexical_qualname, "Root");
    assert_eq!(base.source_digest, Fingerprint::digest(changed));
    assert_eq!(
        imported.inheritance.linearized_bases.last(),
        Some(&BaseReference::Builtin(BuiltinType::Object))
    );
    Ok(())
}

#[test]
fn offline_check_keeps_dataclass_self_fields_and_native_receiver_policy() -> Result<()> {
    use soac_contracts::{AnnotationOrigin, FieldKind, NominalBindingOwner, ParameterKind};

    let fixture = Fixture::new("")?;
    let marker = fixture
        .project
        .join("dataclass-self-source-was-imported.txt");
    fs::write(
        fixture.project.join("nominal_dataclass_support.py"),
        "class Target: pass\ndef post(value): pass\n",
    )?;
    let mut source = r#"# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import InitVar, KW_ONLY, dataclass, field
from typing import ClassVar
from pathlib import Path
from nominal_dataclass_support import Target, post
@dataclass
class Record:
    self: InitVar[Target]
    payload: Target
    def __post_init__(self, seed):
        post(seed)
@dataclass
class Omitted:
    self: int = field(init=False)
@dataclass
class Shared:
    self: ClassVar[int] = 0
@dataclass
class Child(Shared):
    value: int
@dataclass
class Marker:
    self: KW_ONLY
    value: int
@dataclass
class Ordinary:
    self = 0
@dataclass
class FieldNames:
    self: int
    arg0: int
    arg0_: int
"#
    .to_owned();
    source.push_str(&format!(
        "Path({}).write_text('imported')\n",
        serde_json::to_string(&marker)?
    ));
    fs::write(fixture.project.join("model.py"), &source)?;
    let (_, deployment) = fixture.run()?;
    assert!(!marker.exists(), "offline analysis must not execute source");
    let facts = fixture.facts(&deployment)?;
    let class = |name| {
        facts
            .classes
            .iter()
            .find(|class| class.identity.lexical_qualname == name)
            .unwrap()
    };
    for (name, expected) in [
        ("Record", "__dataclass_self__"),
        ("Omitted", "__dataclass_self__"),
        ("Shared", "__dataclass_self__"),
        ("Child", "__dataclass_self__"),
        ("Marker", "self"),
        ("Ordinary", "self"),
        ("FieldNames", "__dataclass_self__"),
    ] {
        let init = class(name)
            .methods
            .iter()
            .find(|method| method.name == "__init__")
            .unwrap();
        assert_eq!(init.signature.parameters[0].name, expected, "{name}");
        assert_eq!(
            init.signature.parameters[0].annotation_origin,
            AnnotationOrigin::Inferred
        );
        assert_eq!(
            init.signature.return_annotation_origin,
            AnnotationOrigin::Inferred
        );
    }
    let record = class("Record");
    let init = record
        .methods
        .iter()
        .find(|method| method.name == "__init__")
        .unwrap();
    let field = record
        .instance_fields
        .iter()
        .find(|field| field.name == "self")
        .unwrap();
    assert_eq!(field.field_kind, FieldKind::InitOnly);
    assert_eq!(init.signature.parameters[1].name, "self");
    assert_eq!(
        init.signature.parameters[1].annotation_origin,
        AnnotationOrigin::Explicit
    );
    assert_eq!(init.signature.parameters[1].value_type, field.value_type);
    let reference = field.annotation_reference().unwrap();
    assert!(facts.nominal_bindings.iter().any(|leaf| {
        matches!(&leaf.owner, NominalBindingOwner::Field { field } if field == &reference)
    }));
    let replace = class("FieldNames")
        .methods
        .iter()
        .find(|method| method.name == "__replace__")
        .unwrap();
    assert_eq!(
        replace.signature.parameters[0].kind,
        ParameterKind::PositionalOnly
    );
    assert_eq!(
        replace
            .signature
            .parameters
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect::<Vec<_>>(),
        ["arg0__", "self", "arg0", "arg0_"]
    );

    let previous = fs::read(fixture.project.join("deployment.json"))?;
    fs::write(
        fixture.project.join("model.py"),
        "# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\n@dataclass\nclass Conflict:\n    self: int\n    __dataclass_self__: int\n",
    )?;
    assert!(
        check(fixture.options()).is_err(),
        "the real duplicate native receiver remains invalid"
    );
    assert_eq!(fs::read(fixture.project.join("deployment.json"))?, previous);
    Ok(())
}

#[test]
fn offline_source_surrogate_escape_never_replaces_startup_authority() -> Result<()> {
    let fixture = Fixture::new(
        "# soac: module(strict_assign=true, checked_attr=true)\ndef value(): return 'valid'\n",
    )?;
    fixture.run()?;
    let original = fs::read(fixture.project.join("deployment.json"))?;
    for body in [
        r#"def value(): return '\ud800'"#,
        r#"def value(arg): return f'{arg:\ud800}'"#,
        r#"def value(arg): return t'\ud800{arg}'"#,
        r#"from typing import Literal
def accept(value: Literal['\ud800']) -> Literal['\ud800']: return value"#,
    ] {
        fs::write(
            fixture.project.join("model.py"),
            format!("# soac: module(strict_assign=true, checked_attr=true)\n{body}\n"),
        )?;
        let error = check(fixture.options()).expect_err("unsupported source cannot be signed");
        assert!(
            error
                .to_string()
                .contains("unsupported Unicode surrogate escape U+D800")
        );
        assert_eq!(fs::read(fixture.project.join("deployment.json"))?, original);
    }
    Ok(())
}

#[test]
fn offline_source_imported_surrogate_alias_never_signs_replacement_literal() -> Result<()> {
    let fixture = Fixture::new(
        "# soac: module(strict_assign=true, checked_attr=true)\nfrom dependency import Alias\ndef accept(value: Alias) -> Alias: return value\n",
    )?;
    fs::write(
        fixture.project.join("dependency.py"),
        "from typing import Literal\nAlias = Literal['\\ud800']\n",
    )?;
    let (_, deployment) = fixture.run()?;
    let facts = fixture.facts(&deployment)?;
    let function = facts
        .functions
        .iter()
        .find(|function| function.identity.lexical_qualname == "accept")
        .unwrap();
    assert_eq!(
        function.signature.parameters[0].value_type,
        StaticType::Unknown
    );
    assert_eq!(function.signature.return_type, StaticType::Unknown);
    assert!(
        function
            .signature
            .uncertainty
            .contains(&soac_contracts::UncertaintyReason::Unknown)
    );

    // The same ordinary dependency with an actual replacement character is
    // supported. Its new source invalidates the old snapshot and can produce
    // the correct exact literal, not a blanket rejection of U+FFFD.
    fs::write(
        fixture.project.join("dependency.py"),
        "from typing import Literal\nAlias = Literal['�']\n",
    )?;
    assert!(verify_analysis_inputs(&deployment.analysis_inputs).is_err());
    let (_, updated) = fixture.run()?;
    let facts = fixture.facts(&updated)?;
    let function = facts
        .functions
        .iter()
        .find(|function| function.identity.lexical_qualname == "accept")
        .unwrap();
    assert_eq!(
        function.signature.parameters[0].value_type,
        StaticType::Literal(soac_contracts::LiteralValue::Str("�".into()))
    );
    assert_eq!(
        function.signature.return_type,
        StaticType::Literal(soac_contracts::LiteralValue::Str("�".into()))
    );
    Ok(())
}

#[test]
fn offline_source_raw_backslashes_and_replacement_literals_are_distinct() -> Result<()> {
    let fixture = Fixture::new(
        r#"# soac: module(strict_assign=true, checked_attr=true)
from typing import Literal
def accept(value: Literal['�'], raw: Literal[r'\ud800']) -> Literal['\ufffd']:
    return value
"#,
    )?;
    let (_, deployment) = fixture.run()?;
    let facts = fixture.facts(&deployment)?;
    let function = facts
        .functions
        .iter()
        .find(|function| function.identity.lexical_qualname == "accept")
        .unwrap();
    assert_eq!(
        function.signature.parameters[0].value_type,
        StaticType::Literal(soac_contracts::LiteralValue::Str("�".into()))
    );
    assert_eq!(
        function.signature.parameters[1].value_type,
        StaticType::Literal(soac_contracts::LiteralValue::Str(r"\ud800".into()))
    );
    assert_eq!(
        function.signature.return_type,
        StaticType::Literal(soac_contracts::LiteralValue::Str("�".into()))
    );
    Ok(())
}

#[test]
fn offline_check_selected_source_aliases_preserve_real_import_and_owner_identity() -> Result<()> {
    for selected in ["__main__", "entry_alias"] {
        let fixture = Fixture::new("")?;
        let entry_marker = fixture.project.join("entry-was-imported.txt");
        let helper_marker = fixture.project.join("helper-was-imported.txt");
        fs::write(
            fixture.project.join("model.py"),
            format!(
                "# soac: module(strict_assign=true, checked_attr=true)\nfrom pathlib import Path\nclass Payload:\n    pass\nimport helper\nPath({}).write_text('imported')\n",
                serde_json::to_string(&entry_marker)?,
            ),
        )?;
        fs::write(
            fixture.project.join("helper.py"),
            format!(
                "# soac: module(strict_assign=true, checked_attr=true)\nfrom pathlib import Path\nfrom {selected} import Payload\ndef make() -> Payload:\n    return Payload()\nPath({}).write_text('imported')\n",
                serde_json::to_string(&helper_marker)?,
            ),
        )?;
        let mut options = fixture.options();
        options.modules = vec![format!("{selected}=model.py"), "helper=helper.py".into()];
        let (_, deployment) = fixture.run_with_options(options)?;
        assert!(!entry_marker.exists() && !helper_marker.exists());
        let entry = fixture.module_facts(&deployment, selected)?;
        let helper = fixture.module_facts(&deployment, "helper")?;
        let payload = entry
            .classes
            .iter()
            .find(|class| class.identity.lexical_qualname == "Payload")
            .unwrap();
        assert_eq!(payload.identity.module.module_name, selected);
        let make = helper
            .functions
            .iter()
            .find(|function| function.identity.lexical_qualname == "make")
            .unwrap();
        let StaticType::NominalClass(returned) = &make.signature.return_type else {
            panic!("the dependency must resolve the actual selected source class");
        };
        assert_eq!(returned.definition, payload.identity);
        assert_eq!(returned.source_digest, entry.source_digest);
        assert!(
            entry
                .consumed_dependencies
                .iter()
                .all(|dependency| dependency.module != entry.module)
        );
        let backedge = deployment
            .analysis_dependencies
            .iter()
            .find(|dependency| {
                dependency.importer_module == "helper" && dependency.module == entry.module
            })
            .expect("the dependency's backedge names the actual selected source owner");
        assert!(
            matches!(&backedge.source, AnalysisDependencySource::System { path }
            if path == &fixture.project.join("model.py").canonicalize()?)
        );
        assert_eq!(backedge.source_digest, entry.source_digest);
        assert!(
            helper
                .nominal_bindings
                .iter()
                .any(|binding| binding.class.definition == payload.identity
                    && binding.binding.module == helper.module,)
        );
    }
    Ok(())
}

#[test]
fn offline_check_unselected_entry_point_stubs_remain_real_dependencies() -> Result<()> {
    let fixture =
        Fixture::new("# soac: module(strict_assign=true, checked_attr=true)\nimport __main__\n")?;
    let (_, deployment) = fixture.run()?;
    let facts = fixture.facts(&deployment)?;
    let dependency = deployment
        .analysis_dependencies
        .iter()
        .find(|dependency| {
            dependency.importer_module == "model" && dependency.module.module_name == "__main__"
        })
        .expect("the actual unselected stub must not be dropped by spelling");
    assert!(matches!(
        &dependency.source,
        AnalysisDependencySource::Vendored { .. }
    ));
    assert!(
        facts
            .consumed_dependencies
            .iter()
            .any(|consumed| consumed.module == dependency.module
                && consumed.source_digest == dependency.source_digest,)
    );
    Ok(())
}
