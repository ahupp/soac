use std::path::Path;

use anyhow::{Context, Result, bail};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use soac_contracts::{Fingerprint, ResolvedStrictPolicy};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProjectPolicy {
    pub(crate) include: Vec<String>,
    pub(crate) exclude: Vec<String>,
    pub(crate) policy: ResolvedStrictPolicy,
    pub(crate) overrides: Vec<PolicyOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PolicyOverride {
    include: Vec<String>,
    exclude: Vec<String>,
    settings: serde_json::Map<String, Value>,
}

fn patterns(value: Option<Value>, default: &[&str]) -> Result<Vec<String>> {
    match value {
        Some(value) => {
            Ok(serde_json::from_value(value).context("strict selection must be a string list")?)
        }
        None => Ok(default.iter().map(|pattern| (*pattern).into()).collect()),
    }
}

fn overlay(
    base: &ResolvedStrictPolicy,
    settings: &serde_json::Map<String, Value>,
) -> Result<ResolvedStrictPolicy> {
    let mut value = serde_json::to_value(base)?;
    let object = value.as_object_mut().expect("policy object");
    for (name, setting) in settings {
        if name == "adapters" {
            let adapters = setting
                .as_object()
                .context("strict adapters must be a table")?;
            let existing = object
                .get_mut(name)
                .and_then(Value::as_object_mut)
                .expect("adapter policy");
            for (name, adapter) in adapters {
                existing.insert(name.clone(), adapter.clone());
            }
        } else {
            object.insert(name.clone(), setting.clone());
        }
    }
    Ok(serde_json::from_value(value).context("invalid strict language policy")?)
}

fn matches(include: &[String], exclude: &[String], path: &Path) -> Result<bool> {
    fn compile(patterns: &[String]) -> Result<GlobSet> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            builder.add(Glob::new(pattern)?);
        }
        Ok(builder.build()?)
    }
    Ok(compile(include)?.is_match(path) && !compile(exclude)?.is_match(path))
}

impl ProjectPolicy {
    pub(crate) fn parse(source: &str) -> Result<Self> {
        let pyproject: toml::Value = toml::from_str(source)?;
        let settings = pyproject
            .get("tool")
            .and_then(|tool| tool.get("soac"))
            .and_then(|soac| soac.get("strict"))
            .context("project requires an explicit [tool.soac.strict] policy")?;
        let value = serde_json::to_value(settings)?;
        let mut settings = value
            .as_object()
            .context("strict policy must be a table")?
            .clone();
        let include = patterns(settings.remove("include"), &["**/*.py"])?;
        let exclude = patterns(
            settings.remove("exclude"),
            &[
                ".git/**",
                ".jj/**",
                ".venv/**",
                "vendor/**",
                "work/**",
                "target/**",
            ],
        )?;
        let mut overrides = Vec::new();
        if let Some(value) = settings.remove("overrides") {
            for item in value
                .as_array()
                .context("strict overrides must be an array of tables")?
            {
                let mut settings = item
                    .as_object()
                    .context("strict override must be a table")?
                    .clone();
                let include = patterns(settings.remove("include"), &[])?;
                let exclude = patterns(settings.remove("exclude"), &[])?;
                if include.is_empty() {
                    bail!("strict override requires include patterns");
                }
                overrides.push(PolicyOverride {
                    include,
                    exclude,
                    settings,
                });
            }
        }
        let policy = overlay(&ResolvedStrictPolicy::default(), &settings)?;
        let result = Self {
            include,
            exclude,
            policy,
            overrides,
        };
        // Validate every override even when no currently selected source matches it.
        for entry in &result.overrides {
            overlay(&result.policy, &entry.settings)?;
            matches(&entry.include, &entry.exclude, Path::new(""))?;
        }
        matches(&result.include, &result.exclude, Path::new(""))?;
        Ok(result)
    }

    pub(crate) fn for_path(&self, relative: &Path) -> Result<Option<ResolvedStrictPolicy>> {
        if !matches(&self.include, &self.exclude, relative)? {
            return Ok(None);
        }
        let mut policy = self.policy.clone();
        for entry in &self.overrides {
            if matches(&entry.include, &entry.exclude, relative)? {
                policy = overlay(&policy, &entry.settings)?;
            }
        }
        Ok(Some(policy))
    }

    pub(crate) fn fingerprint(&self) -> Result<Fingerprint> {
        Ok(Fingerprint::digest(serde_json::to_vec(self)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_contracts::CheckedFieldPolicy;

    #[test]
    fn selection_and_overrides_resolve_one_shared_language_policy() -> Result<()> {
        let policy = ProjectPolicy::parse(
            r#"
[tool.soac.strict]
include = ["services/**"]
exclude = ["services/generated/**"]
[[tool.soac.strict.overrides]]
include = ["services/values/**"]
checked_fields = "supported_annotations"
"#,
        )?;
        assert!(policy.for_path(Path::new("unselected.py"))?.is_none());
        assert!(
            policy
                .for_path(Path::new("services/generated/a.py"))?
                .is_none()
        );
        assert_eq!(
            policy
                .for_path(Path::new("services/a.py"))?
                .unwrap()
                .checked_fields,
            CheckedFieldPolicy::Disabled
        );
        assert_eq!(
            policy
                .for_path(Path::new("services/values/a.py"))?
                .unwrap()
                .checked_fields,
            CheckedFieldPolicy::SupportedAnnotations
        );
        Ok(())
    }

    #[test]
    fn rejects_unknown_and_manual_per_class_policy() {
        assert!(ProjectPolicy::parse("[tool.soac.strict]\nstrict_classes=['Model']").is_err());
        assert!(ProjectPolicy::parse("[tool.soac.strict]\nchecked_fields='sometimes'").is_err());
    }

    #[test]
    fn rejects_removed_call_check_policy_in_defaults_and_unmatched_overrides() {
        for (key, value) in [
            ("checked_parameters", "supported_annotations"),
            ("checked_returns", "disabled"),
            ("parameter_failure", "type_error"),
            ("return_failure", "type_error"),
        ] {
            for prefix in [
                "[tool.soac.strict]\n",
                "[tool.soac.strict]\n[[tool.soac.strict.overrides]]\ninclude=['not-selected/**']\n",
            ] {
                assert!(
                    ProjectPolicy::parse(&format!("{prefix}{key}='{value}'\n")).is_err(),
                    "retired call policy must not be accepted: {key}"
                );
            }
        }
    }
}
