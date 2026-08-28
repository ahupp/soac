//! Resolve source comments without importing Python or consulting a strictness
//! config file. Every consulted package file (including absence) is observed.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use ruff_db::system::System;
use soac_contracts::{ClassPolicyOverride, ResolvedStrictPolicy, SourceRange};
use soac_source::{SoacDirectiveTarget, parse_soac_directives};

use crate::{AnalysisSystem, system_path};

#[derive(Clone, Copy, Debug, Default)]
struct Settings {
    strict_assign: Option<bool>,
    checked_attr: Option<bool>,
}

impl Settings {
    fn apply(self, policy: &mut ResolvedStrictPolicy) {
        if let Some(value) = self.strict_assign {
            policy.strict_assign = value;
        }
        if let Some(value) = self.checked_attr {
            policy.checked_attr = value;
        }
    }
}

#[derive(Clone, Debug, Default)]
struct SourceRules {
    package: Settings,
    module: Settings,
    classes: Vec<ClassPolicyOverride>,
}

pub(crate) struct ProjectPolicy {
    source_root: PathBuf,
    sources: BTreeMap<PathBuf, Option<SourceRules>>,
}

impl ProjectPolicy {
    pub(crate) fn new(source_root: PathBuf) -> Self {
        Self {
            source_root,
            sources: BTreeMap::new(),
        }
    }

    fn rules(&mut self, path: &Path, system: &AnalysisSystem) -> Result<Option<SourceRules>> {
        if let Some(rules) = self.sources.get(path) {
            return Ok(rules.clone());
        }
        let source = match system.read_to_string(system_path(path)?) {
            Ok(source) => source,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.sources.insert(path.to_owned(), None);
                return Ok(None);
            }
            Err(error) => {
                return Err(error).with_context(|| format!("read rules in {}", path.display()));
            }
        };
        let parsed = ruff_python_parser::parse_module(&source)
            .with_context(|| format!("parse policy source {}", path.display()))?;
        let directives = parse_soac_directives(
            &source,
            parsed.tokens(),
            parsed.suite(),
            path.file_name().is_some_and(|name| name == "__init__.py"),
        )
        .with_context(|| format!("invalid SOAC comment in {}", path.display()))?;
        let mut rules = SourceRules::default();
        for directive in directives {
            let settings = Settings {
                strict_assign: directive.strict_assign,
                checked_attr: directive.checked_attr,
            };
            match directive.target {
                SoacDirectiveTarget::Package => rules.package = settings,
                SoacDirectiveTarget::Module => rules.module = settings,
                SoacDirectiveTarget::Class { class_range } => {
                    if let Some(checked_attr) = directive.checked_attr {
                        rules.classes.push(ClassPolicyOverride {
                            class_range: SourceRange::new(
                                class_range.start().into(),
                                class_range.end().into(),
                            ),
                            checked_attr,
                        });
                    }
                }
            }
        }
        rules.classes.sort();
        self.sources.insert(path.to_owned(), Some(rules.clone()));
        Ok(Some(rules))
    }

    pub(crate) fn for_path(
        &mut self,
        path: &Path,
        system: &AnalysisSystem,
    ) -> Result<ResolvedStrictPolicy> {
        ensure!(
            path.starts_with(&self.source_root),
            "source is outside its import root"
        );
        let mut ancestors = Vec::new();
        let mut directory = path.parent().context("source has no parent")?;
        loop {
            ancestors.push(directory.to_owned());
            if directory == self.source_root {
                break;
            }
            directory = directory
                .parent()
                .context("source is outside its import root")?;
        }
        let mut policy = ResolvedStrictPolicy::default();
        for directory in ancestors.into_iter().rev() {
            if let Some(rules) = self.rules(&directory.join("__init__.py"), system)? {
                // Only package settings flow down. An __init__.py module rule
                // changes that module alone, never the package descendants.
                rules.package.apply(&mut policy);
            }
        }
        let rules = self
            .rules(path, system)?
            .context("selected source is missing")?;
        rules.module.apply(&mut policy);
        policy.class_overrides = rules.classes;
        Ok(policy)
    }
}

/// Other ty/project settings remain supported. Retired strictness tables must
/// not silently act as a second authority or appear to configure new rules.
pub(crate) fn reject_config_policy(source: &str) -> Result<()> {
    let config: toml::Value = toml::from_str(source)?;
    ensure!(
        config
            .get("tool")
            .and_then(|tool| tool.get("soac"))
            .and_then(|soac| soac.get("strict"))
            .is_none(),
        "[tool.soac.strict] is no longer a policy source; use # soac: package(...), module(...), and class(...) comments",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_contracts::{AnalysisInputState, verify_analysis_inputs};
    use std::fs;

    #[test]
    fn package_module_and_class_rules_compose_without_crossing_scopes() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().canonicalize()?;
        fs::create_dir_all(root.join("pkg/child"))?;
        fs::write(
            root.join("pkg/__init__.py"),
            "# soac: package(strict_assign=true, checked_attr=true)\n# soac: module(strict_assign=false)\n",
        )?;
        fs::write(
            root.join("pkg/child/__init__.py"),
            "# soac: package(checked_attr=false)\n",
        )?;
        let source = "# soac: module(strict_assign=false)\n# soac: class(checked_attr=true)\nclass C:\n    class Nested: pass\nclass D: pass\n";
        fs::write(root.join("pkg/child/model.py"), source)?;
        let system = AnalysisSystem::new(system_path(&root)?);
        let mut policy = ProjectPolicy::new(root.clone());
        let parent = policy.for_path(&root.join("pkg/__init__.py"), &system)?;
        assert!(!parent.strict_assign);
        assert!(parent.checked_attr);
        let child = policy.for_path(&root.join("pkg/child/__init__.py"), &system)?;
        assert!(child.strict_assign);
        assert!(!child.checked_attr);
        let model = policy.for_path(&root.join("pkg/child/model.py"), &system)?;
        assert!(!model.strict_assign && !model.checked_attr);
        assert!(model.is_selected());
        assert_eq!(model.class_overrides.len(), 1);
        assert!(model.checked_attributes(model.class_overrides[0].class_range));
        assert!(!model.checked_attributes(SourceRange::new(0, 0)));
        Ok(())
    }

    #[test]
    fn consulted_package_sources_and_absence_are_authenticated() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().canonicalize()?;
        fs::create_dir(root.join("pkg"))?;
        fs::write(
            root.join("pkg/model.py"),
            "# soac: module(checked_attr=true)\nclass C: pass\n",
        )?;
        let system = AnalysisSystem::new(system_path(&root)?);
        ProjectPolicy::new(root.clone()).for_path(&root.join("pkg/model.py"), &system)?;
        let (inputs, _) = system.snapshot()?;
        let missing = root.join("pkg/__init__.py");
        assert!(inputs.iter().any(
            |input| input.path == missing && matches!(input.state, AnalysisInputState::Missing)
        ));
        fs::write(missing, "# soac: package(strict_assign=true)\n")?;
        assert!(verify_analysis_inputs(&inputs).is_err());
        Ok(())
    }

    #[test]
    fn ordinary_defaults_and_retired_configuration_are_unambiguous() -> Result<()> {
        assert!(!ResolvedStrictPolicy::default().is_selected());
        reject_config_policy("[tool.ty]\n")?;
        reject_config_policy("literal='[tool.soac.strict]'\n")?;
        assert!(reject_config_policy("[tool.soac.strict]\n").is_err());
        assert!(
            reject_config_policy("[tool.soac.strict.overrides]\nchecked_fields='disabled'\n")
                .is_err()
        );
        Ok(())
    }
}
